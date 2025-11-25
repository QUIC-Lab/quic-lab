//! Template probe showing how to plug a custom application protocol
//! on top of the shared QUIC transport in `crates/core`.
//!
//! How to use this file:
//! 1. Copy it as `template.rs` (or rename it to your probe name).
//! 2. Replace the logic inside `TemplateApp` with your own protocol
//!    implementation (e.g., MASQUE, custom H3 variant, fuzzing probe).
//! 3. Extend `TemplateState` and `TemplateResult` with the metrics you
//!    want to collect.
//! 4. From `runner/src/main.rs`, call `probes::template::probe(...)`
//!    instead of `probes::h3::probe(...)`.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::Result;
use core::config::{ConnectionConfig, GeneralConfig, IOConfig, SchedulerConfig};
use core::recorder::Recorder;
use core::resolver::resolve_targets;
use core::throttle::RateLimit;
use core::transport::quic::{run_probe, AppProtocol};
use log::{debug, error};
use serde::Serialize;
use tquic::Connection;

/// Shared per-connection state that the application logic updates and
/// the outer `probe()` function later serialises via `Recorder`.
#[derive(Debug, Default)]
struct TemplateState {
    /// QUIC trace identifier as assigned by tquic (stable per connection).
    trace_id: Option<String>,

    /// Whether the QUIC handshake ever reached the "established" state.
    handshake_ok: bool,
    // Add your own fields here, for example:
    // pub first_stream_id: Option<u64>,
    // pub bytes_read: u64,
    // pub custom_flag: bool,
}

/// Example application protocol implementation.
/// Replace the method bodies with your own logic.
struct TemplateApp {
    host: String,
    shared: Arc<Mutex<TemplateState>>,
}

impl TemplateApp {
    fn new(host: &str, shared: Arc<Mutex<TemplateState>>) -> Self {
        Self {
            host: host.to_string(),
            shared,
        }
    }
}

impl AppProtocol for TemplateApp {
    fn on_connected(&mut self, conn: &mut Connection) {
        // Called once the QUIC handshake has completed.
        {
            let mut st = self.shared.lock().unwrap();
            st.trace_id = Some(conn.trace_id().to_string());
            st.handshake_ok = true;
        }

        debug!("[{}] template: connection established", self.host);

        // TODO: send your first request/frames here.
        //
        // Typical pattern:
        //   - open a uni-/bidirectional stream via conn.stream_open_...(...)
        //   - send an initial request via conn.stream_send(...)
        //   - rely on on_stream_readable/on_stream_writable to drive the rest
    }

    fn on_stream_readable(&mut self, conn: &mut Connection, stream_id: u64) {
        debug!(
            "[{}] template: stream {} became readable",
            self.host, stream_id
        );

        // TODO: read from the stream and update shared state as needed.
        //
        // Example sketch:
        //
        // let mut buf = [0u8; 4096];
        // loop {
        //     match conn.stream_recv(stream_id, &mut buf) {
        //         Ok((0, _fin)) => break, // no more data
        //         Ok((n, fin)) => {
        //             // process buf[..n], maybe update TemplateState
        //             if fin {
        //                 break;
        //             }
        //         }
        //         Err(e) => {
        //             // tquic::Error::Done means "no more data for now".
        //             debug!("[{}] stream_recv error on {}: {:?}", self.host, stream_id, e);
        //             break;
        //         }
        //     }
        // }
    }

    fn on_stream_writable(&mut self, _conn: &mut Connection, _stream_id: u64) {
        // Optional: drive streaming writes from here if you have request
        // bodies or other application data to send.
    }

    fn on_stream_closed(&mut self, _conn: &mut Connection, stream_id: u64) {
        debug!("[{}] template: stream {} closed", self.host, stream_id);
    }

    fn on_conn_closed(&mut self, conn: &mut Connection) {
        debug!(
            "[{}] template: connection closed (established={}, local={:?}, peer={:?})",
            self.host,
            conn.is_established(),
            conn.local_error(),
            conn.peer_error()
        );

        // Optional: capture final error information into the shared state.
        let mut st = self.shared.lock().unwrap();
        if st.trace_id.is_none() {
            st.trace_id = Some(conn.trace_id().to_string());
        }
    }
}

/// Minimal example of a per-host result that is written via `Recorder`.
/// Extend this struct with whatever fields you need for your evaluation.
#[derive(Debug, Serialize)]
pub struct TemplateResult {
    pub host: String,
    pub trace_id: Option<String>,
    pub elapsed_ms: u128,
    pub handshake_ok: bool,
    // Add your own serialised fields here, mirroring `TemplateState`.
    // pub bytes_read: u64,
    // pub custom_flag: bool,
}

/// Entry point for this probe, mirroring `h3::probe`.
///
/// This function:
///   * resolves the target host for each configured `ConnectionConfig`,
///   * applies the global rate limit (`RateLimit`),
///   * runs the QUIC handshake plus your `TemplateApp`,
///   * records one `TemplateResult` per host into the `Recorder`.
pub fn probe(
    host: &str,
    scheduler_config: &SchedulerConfig,
    io_config: &IOConfig,
    general_config: &GeneralConfig,
    connection_configs: &[ConnectionConfig],
    rl: &RateLimit,
    recorder: &Recorder,
) -> Result<()> {
    for (idx, att) in connection_configs.iter().enumerate() {
        // Resolve host -> (family, SocketAddr) tuples for this attempt.
        let targets = resolve_targets(host, att.port, att.ip_version)?;

        let mut attempt_succeeded = false;

        for (_fam_eff, addr) in targets {
            // Global RPS / burst control.
            rl.until_ready();

            let t_start = Instant::now();
            let shared = Arc::new(Mutex::new(TemplateState::default()));
            let app = TemplateApp::new(host, shared.clone());

            // Run the QUIC engine + your AppProtocol implementation.
            let res = run_probe(host, &addr, io_config, general_config, att, recorder, app);
            let elapsed_ms = t_start.elapsed().as_millis();

            // Snapshot the state as seen by the application logic.
            let st = shared.lock().unwrap();
            let record = TemplateResult {
                host: host.to_string(),
                trace_id: st.trace_id.clone(),
                elapsed_ms,
                handshake_ok: st.handshake_ok,
                // fill in additional fields here
            };

            // Use the trace_id as key when available; fall back to the host.
            let key = record.trace_id.as_deref().unwrap_or(host);

            if let Err(e) = recorder.write_for_key(key, &record) {
                error!(
                    "[{}] template: failed to write recorder record for {}: {e}",
                    host, key
                );
            }

            if let Err(e) = res {
                error!("[{}] template: connect {} error: {e:?}", host, addr);
                continue;
            }

            attempt_succeeded = true;
            break;
        }

        if attempt_succeeded {
            break;
        } else if idx + 1 < connection_configs.len() && scheduler_config.inter_attempt_delay_ms > 0
        {
            std::thread::sleep(std::time::Duration::from_millis(
                scheduler_config.inter_attempt_delay_ms,
            ));
        }
    }

    Ok(())
}
