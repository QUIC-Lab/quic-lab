use anyhow::Result;
use quiche::h3;
use rand::RngCore;
use std::fs::OpenOptions;
use std::net::{SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use quiche::h3::NameValue;
use crate::config::DomainConfig;
use crate::types::ProbeOutcome;

/// Execute one QUIC/H3 probe attempt against a resolved peer.
/// Returns a ProbeOutcome indicating whether a family-fallback retry is sensible.
pub fn run(host: &str, peer_addr: SocketAddr, cfg: &DomainConfig) -> Result<ProbeOutcome> {
    println!("==> {}:{} {}", host, cfg.port, cfg.path);

    let socket = UdpSocket::bind(if peer_addr.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    })?;
    socket.connect(peer_addr)?;
    socket.set_nonblocking(true)?;

    // --- QUIC config
    let mut qcfg = quiche::Config::new(quiche::PROTOCOL_VERSION)?;

    // ALPN
    let alpn_wire: Vec<Vec<u8>> = cfg.alpn.iter().map(|s| s.as_bytes().to_vec()).collect();
    let alpn_refs: Vec<&[u8]> = alpn_wire.iter().map(|v| v.as_slice()).collect();
    qcfg.set_application_protos(&alpn_refs)?;

    const MAX_DATAGRAM_SIZE: usize = 1350;
    qcfg.set_max_recv_udp_payload_size(MAX_DATAGRAM_SIZE);
    qcfg.set_max_send_udp_payload_size(MAX_DATAGRAM_SIZE);
    qcfg.set_max_idle_timeout(cfg.max_idle_timeout_ms);
    qcfg.set_initial_max_data(cfg.initial_max_data);
    qcfg.set_initial_max_stream_data_bidi_local(cfg.initial_max_stream_data_bidi_local);
    qcfg.set_initial_max_stream_data_bidi_remote(cfg.initial_max_stream_data_bidi_remote);
    qcfg.set_initial_max_stream_data_uni(cfg.initial_max_stream_data_uni);
    qcfg.set_initial_max_streams_bidi(cfg.initial_max_streams_bidi);
    qcfg.set_initial_max_streams_uni(cfg.initial_max_streams_uni);
    qcfg.verify_peer(cfg.verify_peer);

    // Resolve key log file path: env var or default "sslkeylogfile.txt"
    let keylog_path: PathBuf = match std::env::var_os("SSLKEYLOGFILE") {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from("sslkeylogfile.txt"),
    };

    // Tell quiche to emit secrets
    qcfg.log_keys();

    // Random SCID
    let mut scid_bytes = [0u8; quiche::MAX_CONN_ID_LEN];
    rand::rng().fill_bytes(&mut scid_bytes);
    let scid = quiche::ConnectionId::from_ref(&scid_bytes);

    let local_addr = socket.local_addr()?;
    let mut conn = quiche::connect(Some(host), &scid, local_addr, peer_addr, &mut qcfg)?;

    // Always try to open/append the keylog file
    match OpenOptions::new()
        .create(true)
        .append(true)
        .open(&keylog_path)
    {
        Ok(file) => conn.set_keylog(Box::new(file)),
        Err(e) => eprintln!(
            "Failed to open keylog file at {}: {e}",
            keylog_path.display()
        ),
    }

    let mut out = [0u8; MAX_DATAGRAM_SIZE];
    let mut in_buf = [0u8; 65535];

    // Kick initial ClientHello
    if let Ok((write, send_info)) = conn.send(&mut out) {
        let _ = socket.send_to(&out[..write], send_info.to)?;
    }

    let start = Instant::now();
    let handshake_deadline_at = start + Duration::from_millis(cfg.handshake_timeout_ms);
    let overall_deadline_at = start + Duration::from_millis(cfg.overall_timeout_ms);

    let mut h3_conn: Option<h3::Connection> = None;
    let mut printed_http3_yes = false;

    loop {
        match socket.recv(&mut in_buf) {
            Ok(len) => {
                let recv_info = quiche::RecvInfo { from: peer_addr, to: socket.local_addr()? };
                let _ = conn.recv(&mut in_buf[..len], recv_info)?;
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            // Windows UDP ICMP “Port Unreachable” (WSAECONNRESET 10054)
            Err(ref e) if e.kind() == std::io::ErrorKind::ConnectionReset || e.raw_os_error() == Some(10054) => {
                println!("   QUIC handshake: FAILED (UDP reset / port unreachable)");
                println!("   HTTP/3 support: NO");
                return Ok(ProbeOutcome::retryable_fail());
            }
            // Linux ECONNREFUSED (111)
            Err(ref e) if e.raw_os_error() == Some(111) => {
                println!("   QUIC handshake: FAILED (UDP reset / port unreachable)");
                println!("   HTTP/3 support: NO");
                return Ok(ProbeOutcome::retryable_fail());
            }
            // Host unreachable variants
            Err(ref e) if e.raw_os_error() == Some(10065) /* WSAEHOSTUNREACH */ => {
                println!("   QUIC handshake: FAILED (host unreachable)");
                println!("   HTTP/3 support: NO");
                return Ok(ProbeOutcome::retryable_fail());
            }
            Err(ref e) if e.raw_os_error() == Some(113) /* EHOSTUNREACH */ => {
                println!("   QUIC handshake: FAILED (host unreachable)");
                println!("   HTTP/3 support: NO");
                return Ok(ProbeOutcome::retryable_fail());
            }
            Err(e) => return Err(e.into()),
        }

        if conn.is_established() && h3_conn.is_none() {
            let alpn_bytes = conn.application_proto(); // &[u8]
            let alpn_str = String::from_utf8_lossy(alpn_bytes);

            println!(
                "   QUIC handshake: OK (ALPN: {})",
                if alpn_bytes.is_empty() {
                    "-"
                } else {
                    &alpn_str
                }
            );

            // Only do HTTP/3 if ALPN == "h3".
            if alpn_bytes == b"h3" {
                let h3_cfg = h3::Config::new()?;
                let mut h3c = h3::Connection::with_transport(&mut conn, &h3_cfg)?;
                let req = vec![
                    h3::Header::new(b":method", b"GET"),
                    h3::Header::new(b":scheme", b"https"),
                    h3::Header::new(b":authority", host.as_bytes()),
                    h3::Header::new(b":path", cfg.path.as_bytes()),
                    h3::Header::new(b"user-agent", b"h3-probe (quiche)"),
                ];
                let _sid = h3c.send_request(&mut conn, &req, true)?;
                h3_conn = Some(h3c);
            } else {
                println!("   HTTP/3 support: NO (ALPN negotiated to '{}')", alpn_str);
            }
        }

        if let Some(h3c) = h3_conn.as_mut() {
            loop {
                match h3c.poll(&mut conn) {
                    Ok((stream_id, ev)) => {
                        use quiche::h3::Event::*;
                        match ev {
                            Headers { list, .. } => {
                                if !printed_http3_yes {
                                    if let Some(s) = extract_status_from_headers(&list) {
                                        println!("   HTTP/3 support: YES (status {s})");
                                    } else {
                                        println!("   HTTP/3 support: YES (headers)");
                                    }
                                    printed_http3_yes = true;
                                }
                            }
                            Data => {
                                let mut body = [0u8; 4096];
                                while let Ok(n) = h3c.recv_body(&mut conn, stream_id, &mut body) {
                                    if n == 0 {
                                        break;
                                    }
                                }
                            }
                            Finished => {
                                conn.close(true, 0, b"done").ok();
                                break;
                            }
                            GoAway { .. } | Reset { .. } | PriorityUpdate { .. } => {}
                        }
                    }
                    Err(h3::Error::Done) => break,
                    Err(e) => {
                        eprintln!("   HTTP/3 error: {e:?}");
                        break;
                    }
                }
            }
        }

        // Send any pending QUIC packets.
        loop {
            match conn.send(&mut out) {
                Ok((write, send_info)) => {
                    let _ = socket.send_to(&out[..write], send_info.to)?;
                }
                Err(quiche::Error::Done) => break,
                Err(e) => return Err(e.into()),
            }
        }

        // Handle QUIC's internal timeout callbacks.
        if let Some(d) = conn.timeout() {
            if Instant::now().duration_since(start) >= d {
                conn.on_timeout();
            }
        }

        // Exit conditions
        if conn.is_closed() {
            break;
        }
        if Instant::now() >= overall_deadline_at {
            println!("   Result: TIMEOUT (overall)");
            return Ok(ProbeOutcome::retryable_fail()); // treat as retryable for Auto
        }
        if !conn.is_established() && Instant::now() >= handshake_deadline_at {
            println!("   QUIC handshake: FAILED (timeout)");
            println!("   HTTP/3 support: NO");
            return Ok(ProbeOutcome::retryable_fail());
        }

        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    if !conn.is_established() && !printed_http3_yes {
        println!("   Result: QUIC not established → HTTP/3 NO");
        return Ok(ProbeOutcome::nonretryable_fail());
    }

    Ok(ProbeOutcome::success())
}

pub fn extract_status_from_headers(list: &[h3::Header]) -> Option<String> {
    for h in list {
        if h.name() == b":status" {
            return Some(String::from_utf8_lossy(h.value()).to_string());
        }
    }
    None
}