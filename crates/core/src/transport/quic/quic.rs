// Copyright (c) 2023 The TQUIC Authors.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use flate2::write::GzEncoder;
use flate2::Compression;
use std::cell::RefCell;
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use log::debug;
use log::error;
use mio::event::Event;
use tquic::Config;
use tquic::Connection;
use tquic::Endpoint;
use tquic::PacketInfo;
use tquic::TlsConfig;
use tquic::TransportHandler;

use crate::config::{ConnectionConfig, GeneralConfig, IOConfig};
use crate::recorder::Recorder;
use crate::shard2;
use crate::transport::quic::QuicSocket;
use crate::transport::quic::Result;
use crate::types::{BasicStats, MetaRecord};

/// Application protocol hook that runs on top of QUIC.
/// Implementations may drive HTTP/3, HTTP/0.9, or anything else.
pub trait AppProtocol {
    fn on_connected(&mut self, _conn: &mut Connection) {}
    fn on_stream_readable(&mut self, _conn: &mut Connection, _stream_id: u64) {}
    fn on_stream_writable(&mut self, _conn: &mut Connection, _stream_id: u64) {}
    fn on_stream_closed(&mut self, _conn: &mut Connection, _stream_id: u64) {}
    fn on_conn_closed(&mut self, _conn: &mut Connection) {}
}

impl dyn AppProtocol {}

// A simple http client over QUIC.
struct Client {
    /// QUIC endpoint.
    endpoint: Endpoint,

    /// Event poll.
    poll: mio::Poll,

    /// Socket connecting to server.
    sock: Rc<QuicSocket>,

    /// Client context.
    context: Rc<RefCell<ClientContext>>,

    /// Packet read buffer.
    recv_buf: Vec<u8>,
}

impl Client {
    fn new(
        host: &str,
        socket_addr: &SocketAddr,
        io_config: &IOConfig,
        general_config: &GeneralConfig,
        connection_config: &ConnectionConfig,
        recorder: &Recorder,
        app: Box<dyn AppProtocol>,
    ) -> Result<Self> {
        let mut config = Config::new()?;
        config.set_max_idle_timeout(connection_config.max_idle_timeout_ms);
        config.set_initial_max_data(connection_config.initial_max_data);
        config.set_initial_max_stream_data_bidi_local(
            connection_config.initial_max_stream_data_bidi_local,
        );
        config.set_initial_max_stream_data_bidi_remote(
            connection_config.initial_max_stream_data_bidi_remote,
        );
        config.set_initial_max_stream_data_uni(connection_config.initial_max_stream_data_uni);
        config.set_initial_max_streams_bidi(connection_config.initial_max_streams_bidi);
        config.set_initial_max_streams_uni(connection_config.initial_max_streams_uni);
        config.set_max_ack_delay(connection_config.max_ack_delay);
        config.set_active_connection_id_limit(connection_config.active_connection_id_limit);
        config.set_send_udp_payload_size(connection_config.send_udp_payload_size);

        config.enable_multipath(connection_config.enable_multipath);
        config.set_multipath_algorithm(connection_config.multipath_algorithm.parse().unwrap());

        // TLS + ALPN
        let alpn_wire: Vec<Vec<u8>> = connection_config
            .alpn
            .iter()
            .map(|s| s.as_bytes().to_vec())
            .collect();
        let mut tls_config = TlsConfig::new_client_config(alpn_wire, false)?;
        tls_config.set_verify(connection_config.verify_peer);
        config.set_tls_config(tls_config);

        let context = Rc::new(RefCell::new(ClientContext { finish: false }));
        let handlers = ClientHandler::new(
            host,
            socket_addr,
            io_config,
            general_config,
            recorder,
            context.clone(),
            app,
        );

        let poll = mio::Poll::new()?;
        let registry = poll.registry();
        let sock = Rc::new(QuicSocket::new_client_socket(
            socket_addr.is_ipv4(),
            registry,
        )?);

        Ok(Client {
            endpoint: Endpoint::new(Box::new(config), false, Box::new(handlers), sock.clone()),
            poll,
            sock,
            context,
            recv_buf: vec![0u8; connection_config.max_receive_buffer_size],
        })
    }

    fn finish(&self) -> bool {
        let context = self.context.borrow();
        context.finish()
    }

    fn process_read_event(&mut self, event: &Event) -> Result<()> {
        loop {
            if self.context.borrow().finish() {
                break;
            }
            // Read datagram from the socket.
            let (len, local, remote) = match self.sock.recv_from(&mut self.recv_buf, event.token())
            {
                Ok(v) => v,
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::WouldBlock {
                        debug!("socket recv would block");
                        break;
                    }
                    return Err(format!("socket recv error: {:?}", e).into());
                }
            };
            debug!("socket recv recv {} bytes from {:?}", len, remote);

            let pkt_buf = &mut self.recv_buf[..len];
            let pkt_info = PacketInfo {
                src: remote,
                dst: local,
                time: Instant::now(),
            };

            // Process the incoming packet.
            if let Err(e) = self.endpoint.recv(pkt_buf, &pkt_info) {
                error!("recv failed: {:?}", e);
                continue;
            }
        }

        Ok(())
    }
}

struct ClientContext {
    finish: bool,
}

impl ClientContext {
    fn set_finish(&mut self, finish: bool) {
        self.finish = finish
    }

    fn finish(&self) -> bool {
        self.finish
    }
}

struct ClientHandler {
    host: String,
    peer_addr: SocketAddr,
    session_root: PathBuf,
    keylog_root: PathBuf,
    qlog_root: PathBuf,
    recorder: Recorder,
    context: Rc<RefCell<ClientContext>>,
    app: Box<dyn AppProtocol>,
}

impl ClientHandler {
    fn new(
        host: &str,
        peer_addr: &SocketAddr,
        io_config: &IOConfig,
        general_config: &GeneralConfig,
        recorder: &Recorder,
        context: Rc<RefCell<ClientContext>>,
        app: Box<dyn AppProtocol>,
    ) -> Self {
        let base = PathBuf::from(&io_config.out_dir);
        let session_root = if general_config.save_session_files {
            base.join("session_files")
        } else {
            PathBuf::new()
        };
        let keylog_root = if general_config.save_keylog_files {
            base.join("keylog_files")
        } else {
            PathBuf::new()
        };
        let qlog_root = if general_config.save_qlog_files {
            base.join("qlog_files")
        } else {
            PathBuf::new()
        };

        // Create folders if not exist
        let _ = fs::create_dir_all(&session_root);
        let _ = fs::create_dir_all(&keylog_root);
        let _ = fs::create_dir_all(&qlog_root);

        Self {
            host: host.to_string(),
            peer_addr: peer_addr.clone(),
            session_root,
            keylog_root,
            qlog_root,
            recorder: recorder.clone(),
            context,
            app,
        }
    }
}

impl TransportHandler for ClientHandler {
    fn on_conn_created(&mut self, conn: &mut Connection) {
        debug!("{} connection is created", conn.trace_id());
        let id = conn.trace_id().to_string();

        // qlog
        if !self.qlog_root.as_os_str().is_empty() {
            let qdir = shard2(&self.qlog_root, &id);
            let _ = fs::create_dir_all(&qdir);
            let qlog_path = qdir.join(format!("{id}.qlog.ndjson.gz"));
            if let Ok(qlog) = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&qlog_path)
            {
                let gz = GzEncoder::new(qlog, Compression::fast());
                conn.set_qlog(
                    Box::new(gz),
                    "client qlog".into(),
                    format!("host={} id={}", self.host, id),
                );
            } else {
                error!("{} set qlog failed", id);
            }
        }

        // keylog
        if !self.keylog_root.as_os_str().is_empty() {
            let kdir = shard2(&self.keylog_root, &id);
            let _ = fs::create_dir_all(&kdir);
            let keylog_path = kdir.join(format!("{id}.keylog"));
            if let Ok(keylog) = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&keylog_path)
            {
                conn.set_keylog(Box::new(keylog));
            } else {
                error!("{} set key log failed", id);
            }
        }

        // session resume
        if !self.session_root.as_os_str().is_empty() {
            // Stable key needed --> host
            let key = &self.host; // minimal stable key
            let sdir = shard2(&self.session_root, key);
            let _ = fs::create_dir_all(&sdir);
            let session_path = sdir.join(format!("{key}.session"));
            if let Ok(session) = fs::read(&session_path) {
                if let Err(e) = conn.set_session(&session) {
                    error!("{} session resumption failed: {:?}", conn.trace_id(), e);
                }
            }
        }
    }

    fn on_conn_established(&mut self, conn: &mut Connection) {
        debug!("{} connection is established", conn.trace_id());

        // If connection crashes, we still have a session file
        if !self.session_root.as_os_str().is_empty() {
            if let Some(session) = conn.session() {
                let key = &self.host;
                let sdir = shard2(&self.session_root, key);
                let _ = fs::create_dir_all(&sdir);
                let session_path = sdir.join(format!("{key}.session"));
                let _ = fs::write(&session_path, session);
            }
        }

        self.app.on_connected(conn);
    }

    fn on_conn_closed(&mut self, conn: &mut Connection) {
        let id = conn.trace_id().to_string();
        debug!("{} connection is closed", id);
        let mut context = self.context.try_borrow_mut().unwrap();
        context.set_finish(true);

        // Persist session
        if !self.session_root.as_os_str().is_empty() {
            if let Some(session) = conn.session() {
                let key = &self.host;
                let sdir = shard2(&self.session_root, key);
                let _ = fs::create_dir_all(&sdir);
                let session_path = sdir.join(format!("{key}.session"));
                if let Err(e) = fs::write(&session_path, session) {
                    error!("write session failed: {:?}", e);
                }
            }
        }

        // Recorder file
        let s = conn.stats();
        let meta = MetaRecord {
            host: self.host.clone(),
            peer_addr: self.peer_addr.clone(),
            alpn: {
                let v: &[u8] = conn.application_proto();
                if v.is_empty() {
                    None
                } else {
                    Some(String::from_utf8_lossy(v).into_owned())
                }
            },
            handshake_ok: conn.is_established(),
            local_close: conn.local_error().map(|e| format!("{e:?}")),
            peer_close: conn.peer_error().map(|e| format!("{e:?}")),
            enable_multipath: conn.is_multipath(),
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

        self.app.on_conn_closed(conn);
    }

    fn on_stream_created(&mut self, conn: &mut Connection, stream_id: u64) {
        debug!("{} stream {} is created", conn.trace_id(), stream_id);
    }

    fn on_stream_readable(&mut self, conn: &mut Connection, stream_id: u64) {
        self.app.on_stream_readable(conn, stream_id);
    }

    fn on_stream_writable(&mut self, conn: &mut Connection, stream_id: u64) {
        self.app.on_stream_writable(conn, stream_id);
    }

    fn on_stream_closed(&mut self, conn: &mut Connection, stream_id: u64) {
        debug!("{} stream {} is closed", conn.trace_id(), stream_id);
        self.app.on_stream_closed(conn, stream_id);
    }

    fn on_new_token(&mut self, _conn: &mut Connection, _token: Vec<u8>) {}
}

pub fn open_connection(
    host: &str,
    socket_addr: &SocketAddr,
    io_config: &IOConfig,
    general_config: &GeneralConfig,
    connection_config: &ConnectionConfig,
    recorder: &Recorder,
    app: Box<dyn AppProtocol>,
) -> Result<()> {
    // Create client
    let mut client = Client::new(
        host,
        socket_addr,
        io_config,
        general_config,
        connection_config,
        recorder,
        app,
    )?;

    // Connect to server
    client.endpoint.connect(
        client.sock.local_addr(),
        socket_addr.clone(),
        Option::from(host),
        None,
        None,
        None,
    )?;

    // Run event loop
    let mut events = mio::Events::with_capacity(1024);
    loop {
        // Process connections.
        client.endpoint.process_connections()?;
        if client.finish() {
            break;
        }

        client.poll.poll(&mut events, client.endpoint.timeout())?;

        // Process IO events
        for event in events.iter() {
            if event.is_readable() {
                client.process_read_event(event)?;
            }
        }

        // Process timeout events
        // Note: Since `poll()` doesn't clearly tell if there was a timeout when it returns,
        // it is up to the endpoint to check for a timeout and deal with it.
        client.endpoint.on_timeout(Instant::now());
    }
    Ok(())
}
