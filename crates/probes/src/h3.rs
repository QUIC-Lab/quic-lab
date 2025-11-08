use crate::h3::quic::AppProtocol;
use anyhow::Result;
use core::config::{ConnectionConfig, DelayConfig, IOConfig};
use core::recorder::Recorder;
use core::resolver::resolve_targets;
use core::throttle::RateLimit;
use core::transport::quic::quic;
use core::types::{BasicStats, MetaRecord};
use std::net::SocketAddr;

use log::{debug, error};
use tquic::h3::connection::Http3Connection;
use tquic::h3::{Header, Http3Config, Http3Event, NameValue};
use tquic::Connection;

/// HTTP/3 app protocol plugged into the QUIC engine.
struct H3App {
    host: String,
    peer_addr: SocketAddr,
    path: String,
    user_agent: String,
    recorder: Recorder,

    h3: Option<Http3Connection>,
    req_stream: Option<u64>,

    // simple state for result extraction
    status: Option<u16>,
    headers_seen: bool,
}

impl H3App {
    fn new(
        host: &str,
        peer_addr: &SocketAddr,
        path: &str,
        user_agent: &str,
        recorder: &Recorder,
    ) -> Self {
        let mut full_path = peer_addr.to_string();
        full_path.push_str(path);
        Self {
            host: host.to_string(),
            peer_addr: *peer_addr,
            path: full_path,
            user_agent: user_agent.to_string(),
            recorder: recorder.clone(),
            h3: None,
            req_stream: None,
            status: None,
            headers_seen: false,
        }
    }
}

impl AppProtocol for H3App {
    fn on_connected(&mut self, conn: &mut Connection) {
        // Initialize H3 over QUIC and send a minimal GET request.
        let h3_cfg = match Http3Config::new() {
            Ok(c) => c,
            Err(e) => {
                error!("http3 config error: {:?}", e);
                let _ = conn.close(true, 0x1, b"h3cfg");
                return;
            }
        };

        let mut h3 = match Http3Connection::new_with_quic_conn(conn, &h3_cfg) {
            Ok(h) => h,
            Err(e) => {
                error!("http3 init error: {:?}", e);
                let _ = conn.close(true, 0x1, b"h3init");
                return;
            }
        };

        let sid = match h3.stream_new(conn) {
            Ok(s) => s,
            Err(e) => {
                error!("http3 stream_new error: {:?}", e);
                let _ = conn.close(true, 0x1, b"h3sid");
                return;
            }
        };

        // Build request headers.
        let headers = [
            Header::new(b":method", b"GET"),
            Header::new(b":scheme", b"https"),
            Header::new(b":authority", self.host.as_bytes()),
            Header::new(b":path", self.path.as_bytes()),
            Header::new(b"user-agent", self.user_agent.as_bytes()),
            Header::new(b"accept", b"*/*"),
        ];

        if let Err(e) = h3.send_headers(conn, sid, &headers, true /* fin: no body */) {
            error!("send_headers error: {:?}", e);
            let _ = conn.close(true, 0x1, b"hdr");
            return;
        }

        self.h3 = Some(h3);
        self.req_stream = Some(sid);
    }

    fn on_stream_readable(&mut self, conn: &mut Connection, _stream_id: u64) {
        // Drive H3 by polling events until Done.
        let Some(h3) = self.h3.as_mut() else {
            return;
        };

        loop {
            let ev = match h3.poll(conn) {
                Ok(ev) => ev,
                Err(e) => {
                    // Http3Error::Done => no more events now.
                    debug!("h3.poll: {:?}", e);
                    break;
                }
            };

            let (sid, event) = ev;
            match event {
                Http3Event::Headers { headers, fin } => {
                    // extract :status
                    for hdr in headers.iter() {
                        if hdr.name() == b":status" {
                            if let Ok(s) = std::str::from_utf8(hdr.value()) {
                                if let Ok(code) = s.parse::<u16>() {
                                    self.status = Some(code);
                                }
                            }
                        }
                    }
                    self.headers_seen = true;

                    // if headers carried FIN, there is no body
                    if fin {
                        let _ = h3.stream_close(conn, sid);
                        let _ = conn.close(true, 0x00, b"ok");
                    }
                }

                Http3Event::Data => {
                    // drain body
                    let mut buf = [0u8; 8192];
                    loop {
                        match h3.recv_body(conn, sid, &mut buf) {
                            Ok(0) => break,
                            Ok(_n) => { /* discard */ }
                            Err(_e) => break, // Done or error
                        }
                    }
                }

                Http3Event::Finished => {
                    let _ = h3.stream_close(conn, sid);
                    let _ = conn.close(true, 0x00, b"ok");
                }

                _ => { /* ignore other events for probing */ }
            }
        }
    }

    fn on_stream_writable(&mut self, _conn: &mut Connection, _stream_id: u64) {
        // Not used for GET without body.
    }

    fn on_stream_closed(&mut self, _conn: &mut Connection, _stream_id: u64) {}

    fn on_conn_closed(&mut self, _conn: &mut Connection) {
        let id = _conn.trace_id().to_string();

        let s = _conn.stats();
        let meta = MetaRecord {
            host: self.host.clone(),
            peer_addr: self.peer_addr,
            alpn: {
                let v: &[u8] = _conn.application_proto();
                if v.is_empty() {
                    None
                } else {
                    Some(String::from_utf8_lossy(v).into_owned())
                }
            },
            handshake_ok: _conn.is_established(),
            local_close: _conn.local_error().map(|e| format!("{e:?}")),
            peer_close: _conn.peer_error().map(|e| format!("{e:?}")),
            stats: Some(BasicStats {
                bytes_sent: s.sent_bytes,
                bytes_recv: s.recv_bytes,
                bytes_lost: s.lost_bytes,
                packets_sent: s.sent_count,
                packets_recv: s.recv_count,
                packets_lost: s.lost_count,
            }),
        };

        if let Err(e) = self.recorder.write_for_key(&id, &meta) {
            log::error!("write result for {} failed: {}", id, e);
        }

        debug!("h3 finished, status = {:?}", self.status);
    }
}

/// Try a sequence of connection configs; stop at first success. Every config is attempted.
pub fn probe(
    host: &str,
    io_config: &IOConfig,
    connection_configs: &[ConnectionConfig],
    delay: &DelayConfig,
    rl: &RateLimit,
    recorder: &Recorder,
) -> Result<()> {
    for (idx, att) in connection_configs.iter().enumerate() {
        // Centralized resolution
        let targets = resolve_targets(host, att.port, att.ip_version)?;

        let mut attempt_succeeded = false;

        for (_fam_eff, addr) in targets {
            rl.until_ready();

            // Build the HTTP/3 app and open a QUIC connection that will drive it.
            let app = Box::new(H3App::new(
                host,
                &addr,
                &att.path,
                &att.user_agent,
                recorder,
            ));

            // NOTE: business logic of core’s event loop remains unchanged.
            if let Err(e) = quic::open_connection(host, &addr, io_config, att, app) {
                error!("[{}] connect {} err: {e:?}", host, addr);
                continue;
            }

            // If we reached here cleanly, count as success for this address.
            attempt_succeeded = true;
            // For probing you may choose to continue to the other family, but we stop on first success.
            break;
        }

        if attempt_succeeded {
            break;
        } else if idx + 1 < connection_configs.len() && delay.inter_attempt_delay_ms > 0 {
            std::thread::sleep(std::time::Duration::from_millis(
                delay.inter_attempt_delay_ms,
            ));
        }
    }

    Ok(())
}
