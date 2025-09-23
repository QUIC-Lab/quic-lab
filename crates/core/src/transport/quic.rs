use anyhow::Result;
use std::cell::RefCell;
use std::net::{SocketAddr, UdpSocket};
use std::rc::Rc;
use std::time::{Duration, Instant};

use tquic::{
    endpoint::Endpoint,
    Config as QuicConfig,
    Connection,
    PacketInfo,
    PacketSendHandler,
    TlsConfig,
    TransportHandler,
    Error as QuicError,
    MultipathAlgorithm,
    h3::{
        connection::Http3Connection,
        Http3Config,
        Http3Event,
        Header,
        NameValue,
        Http3Error,
    },
};

use crate::types::{
    ConnectionConfigConfig,
    Http3Result,
    IpVersion,
    MinimalConnectionConfigCfg,
    ProbeOutcome,
    ProbeRecord,
};

/// Map string -> MultipathAlgorithm (tquic 1.6 public variants).
fn parse_mpath_algo(s: &str) -> Option<MultipathAlgorithm> {
    match s {
        "minrtt" | "min-rtt" => Some(MultipathAlgorithm::MinRtt),
        "redundant" | "dup"  => Some(MultipathAlgorithm::Redundant),
        "roundrobin" | "rr"  => Some(MultipathAlgorithm::RoundRobin),
        _ => None,
    }
}

/// Public entry (compat): alias to run_attempt.
pub fn run(
    host: &str,
    peer_addr: SocketAddr,
    fam: IpVersion,
    cfg: &ConnectionConfigConfig,
) -> Result<(ProbeRecord, ProbeOutcome)> {
    run_connection_config(host, peer_addr, fam, cfg)
}

/// Drive a single QUIC(+H3) attempt using tquic.
pub fn run_connection_config(
    host: &str,
    peer_addr: SocketAddr,
    fam: IpVersion,
    cfg: &ConnectionConfigConfig,
) -> Result<(ProbeRecord, ProbeOutcome)> {
    const MAX_DGRAM: usize = 1350;

    // UDP socket
    let socket = UdpSocket::bind(if peer_addr.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" })?;
    socket.connect(peer_addr)?;
    socket.set_nonblocking(true)?;

    // ---- tquic Config
    let mut qcfg = QuicConfig::new()?;
    qcfg.set_max_idle_timeout(cfg.max_idle_timeout_ms);
    qcfg.set_max_handshake_timeout(cfg.handshake_timeout_ms);
    qcfg.set_recv_udp_payload_size(MAX_DGRAM as u16);
    qcfg.set_send_udp_payload_size(MAX_DGRAM);

    qcfg.set_initial_max_data(cfg.initial_max_data);
    qcfg.set_initial_max_stream_data_bidi_local(cfg.initial_max_stream_data_bidi_local);
    qcfg.set_initial_max_stream_data_bidi_remote(cfg.initial_max_stream_data_bidi_remote);
    qcfg.set_initial_max_stream_data_uni(cfg.initial_max_stream_data_uni);
    qcfg.set_initial_max_streams_bidi(cfg.initial_max_streams_bidi);
    qcfg.set_initial_max_streams_uni(cfg.initial_max_streams_uni);

    if cfg.multipath {
        qcfg.enable_multipath(true);
        if let Some(algo) = cfg
            .multipath_algorithm
            .as_deref()
            .and_then(|s| parse_mpath_algo(&s.to_ascii_lowercase()))
        {
            qcfg.set_multipath_algorithm(algo);
        }
    }

    // TLS + ALPN
    let alpn_wire: Vec<Vec<u8>> = cfg.alpn.iter().map(|s| s.as_bytes().to_vec()).collect();
    // NOTE: tquic 1.6: new_client_config(alpn, early_data_enabled=false)
    let mut tls = TlsConfig::new_client_config(alpn_wire, false)?;
    tls.set_verify(cfg.verify_peer);
    qcfg.set_tls_config(tls);

    // ---- endpoint + handlers
    let sender = Rc::new(UdpSender { socket });
    let state = Rc::new(RefCell::new(H3State::new(host, &cfg.path)));
    let handler = Rc::new(TransportCb { state: state.clone() });
    let mut endpoint = Endpoint::new(
        Box::new(qcfg),
        /*server=*/ false,
        Box::new(HandlerShim { inner: handler }),
        sender.clone(),
    );

    // Connect (last arg: per-connection Config override -> None)
    let _id = endpoint.connect(
        sender.socket.local_addr()?,
        peer_addr,
        Some(host),
        None,
        None,
        None,
    )?;

    // Timers
    let t_start = Instant::now();
    let handshake_deadline = t_start + Duration::from_millis(cfg.handshake_timeout_ms);
    let overall_deadline   = t_start + Duration::from_millis(cfg.overall_timeout_ms);

    // I/O buffer
    let mut in_buf = vec![0u8; 64 * 1024];

    // Main loop
    loop {
        // 1) read UDP (nonblocking)
        match sender.socket.recv_from(&mut in_buf) {
            Ok((n, from)) => {
                let info = PacketInfo {
                    src: from,
                    dst: sender.socket.local_addr()?,
                    time: Instant::now(),
                };
                endpoint.recv(&mut in_buf[..n], &info)?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e.into()),
        }

        // 2) advance connections
        endpoint.process_connections()?;

        // 3) timeouts
        if let Some(dur) = endpoint.timeout() {
            if Instant::now().duration_since(t_start) >= dur {
                endpoint.on_timeout(Instant::now());
            }
        }

        // 4) exit conditions / deadlines
        {
            let s = state.borrow();
            if s.done {
                break;
            }
            if s.t_handshake_ok_ms.is_none() && Instant::now() >= handshake_deadline {
                drop(s);
                state.borrow_mut().error = Some("handshake timeout".into());
                break;
            }
        }
        if Instant::now() >= overall_deadline {
            state.borrow_mut().error = Some("overall timeout".into());
            break;
        }

        std::thread::sleep(Duration::from_millis(2));
    }

    // Build record
    let s = state.borrow();
    let rec = ProbeRecord {
        host: s.host.clone(),
        fam: match fam {
            IpVersion::Ipv4 => "IPv4",
            IpVersion::Ipv6 => "IPv6",
            IpVersion::Auto => "Auto",
            IpVersion::Both => "Both",
        }
            .to_string(),
        peer_addr: peer_addr.to_string(),

        t_start_ms: 0,
        t_handshake_ok_ms: s.t_handshake_ok_ms,
        t_end_ms: 0,

        alpn: s.alpn.clone(),
        http3: Http3Result {
            attempted: s.h3_attempted,
            status: s.h3_status,
        },

        error: s.error.clone(),

        cfg: MinimalConnectionConfigCfg {
            alpn: cfg.alpn.clone(),
            verify_peer: cfg.verify_peer,
            multipath: cfg.multipath,
            multipath_algorithm: cfg.multipath_algorithm.clone(),
        },
    };

    let outcome = if s.t_handshake_ok_ms.is_some() {
        ProbeOutcome::success()
    } else {
        ProbeOutcome::retryable_fail()
    };

    Ok((rec, outcome))
}

/// UDP sender used by tquic for outbound packets.
struct UdpSender {
    socket: UdpSocket,
}

impl PacketSendHandler for UdpSender {
    fn on_packets_send(&self, pkts: &[(Vec<u8>, PacketInfo)]) -> Result<usize, QuicError> {
        let mut sent = 0usize;
        for (buf, info) in pkts {
            match self.socket.send_to(buf, info.dst) {
                Ok(_) => sent += 1,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(QuicError::IoError(e.to_string())),
            }
        }
        Ok(sent)
    }
}

/// Internal shared state the transport callback updates.
struct H3State {
    host: String,
    path: String,

    // timeline
    t_start: Instant,
    t_handshake_ok_ms: Option<u128>,

    // negotiated
    alpn: Option<String>,

    // http3
    h3: Option<Http3Connection>,
    h3_attempted: bool,
    h3_status: Option<u16>,

    // terminal
    error: Option<String>,
    done: bool,
}

impl H3State {
    fn new(host: &str, path: &str) -> Self {
        Self {
            host: host.to_string(),
            path: path.to_string(),
            t_start: Instant::now(),
            t_handshake_ok_ms: None,
            alpn: None,
            h3: None,
            h3_attempted: false,
            h3_status: None,
            error: None,
            done: false,
        }
    }
}

/// Thin shim so we can hold Rc<TransportCb> but pass Box<dyn TransportHandler>.
struct HandlerShim {
    inner: Rc<TransportCb>,
}

impl TransportHandler for HandlerShim {
    fn on_conn_created(&mut self, _conn: &mut Connection) {}

    fn on_conn_established(&mut self, conn: &mut Connection) {
        self.inner.on_conn_established(conn);
    }

    fn on_conn_closed(&mut self, _conn: &mut Connection) {
        self.inner.on_conn_closed();
    }

    fn on_stream_created(&mut self, _conn: &mut Connection, _sid: u64) {}

    fn on_stream_readable(&mut self, conn: &mut Connection, _sid: u64) {
        self.inner.on_stream_readable(conn);
    }

    fn on_stream_writable(&mut self, _conn: &mut Connection, _sid: u64) {}

    fn on_stream_closed(&mut self, _conn: &mut Connection, _sid: u64) {}

    fn on_new_token(&mut self, _conn: &mut Connection, _token: Vec<u8>) {}
}

/// Real logic lives here, but behind Rc so we can keep a handle to the state.
struct TransportCb {
    state: Rc<RefCell<H3State>>,
}

impl TransportCb {
    fn on_conn_established(&self, conn: &mut Connection) {
        let mut st = self.state.borrow_mut();
        if st.t_handshake_ok_ms.is_none() {
            st.t_handshake_ok_ms = Some(st.t_start.elapsed().as_millis());
        }

        // If ALPN were exposed, you could set st.alpn here.

        // Initialize H3
        let mut h3c = match Http3Connection::new_with_quic_conn(conn, &Http3Config::new().unwrap()) {
            Ok(h) => h,
            Err(e) => {
                st.error = Some(format!("h3 init failed: {e:?}"));
                st.done = true;
                return;
            }
        };
        st.h3_attempted = true;

        // Create a request stream and send headers (GET)
        match h3c.stream_new(conn) {
            Ok(sid) => {
                let headers = vec![
                    Header::new(b":method", b"GET"),
                    Header::new(b":scheme", b"https"),
                    Header::new(b":authority", st.host.as_bytes()),
                    Header::new(b":path", st.path.as_bytes()),
                    Header::new(b"user-agent", b"h3-probe (tquic)"),
                ];
                if let Err(e) = h3c.send_headers(conn, sid, &headers, true) {
                    st.error = Some(format!("h3 send headers error: {e:?}"));
                    st.done = true;
                    return;
                }
                st.h3 = Some(h3c);
            }
            Err(e) => {
                st.error = Some(format!("h3 stream_new error: {e:?}"));
                st.done = true;
            }
        }
    }

    fn on_stream_readable(&self, conn: &mut Connection) {
        // Poll without holding a long-lived mutable borrow to st.h3 to satisfy the borrow checker.
        loop {
            // 1) poll event
            let ev_res = {
                let mut st = self.state.borrow_mut();
                let Some(h3) = st.h3.as_mut() else { return };
                h3.poll(conn)
            };

            // 2) handle event (now we can borrow st again if needed)
            match ev_res {
                Ok((sid, ev)) => match ev {
                    Http3Event::Headers { headers, .. } => {
                        if let Some(code) = extract_status(&headers) {
                            let mut st = self.state.borrow_mut();
                            st.h3_status = Some(code);
                        }
                    }
                    Http3Event::Data => {
                        let mut buf = [0u8; 4096];
                        let mut st = self.state.borrow_mut();
                        if let Some(h3) = st.h3.as_mut() {
                            while let Ok(n) = h3.recv_body(conn, sid, &mut buf) {
                                if n == 0 {
                                    break;
                                }
                            }
                        }
                    }
                    Http3Event::Finished => {
                        let mut st = self.state.borrow_mut();
                        st.done = true;
                        return;
                    }
                    Http3Event::GoAway | Http3Event::PriorityUpdate | Http3Event::Reset(_) => {}
                },
                Err(Http3Error::Done) => break,
                Err(e) => {
                    let mut st = self.state.borrow_mut();
                    st.error = Some(format!("h3 poll error: {e:?}"));
                    st.done = true;
                    break;
                }
            }
        }
    }

    fn on_conn_closed(&self) {
        self.state.borrow_mut().done = true;
    }
}

/// Extract :status from HTTP/3 headers.
fn extract_status(headers: &[Header]) -> Option<u16> {
    for h in headers {
        if h.name() == b":status" {
            if let Ok(s) = std::str::from_utf8(h.value()) {
                if let Ok(code) = s.parse::<u16>() {
                    return Some(code);
                }
            }
        }
    }
    None
}
