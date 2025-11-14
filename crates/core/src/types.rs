use crate::config::ConnectionConfig;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

/// Which IP family to use when probing (config values: "auto", "ipv4", "ipv6").
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IpVersion {
    Auto,
    Ipv4,
    Ipv6,
}

impl Default for IpVersion {
    fn default() -> Self {
        IpVersion::Auto
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ProbeOutcome {
    /// true if failure looked like timeout/ICMP "no QUIC here", so trying the other family makes sense
    pub retryable: bool,
}

impl ProbeOutcome {
    pub fn success() -> Self {
        Self { retryable: false }
    }
    pub fn retryable_fail() -> Self {
        Self { retryable: true }
    }
    pub fn nonretryable_fail() -> Self {
        Self { retryable: false }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Http3Result {
    pub attempted: bool,
    pub status: Option<u16>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProbeRecord {
    pub host: String,
    pub fam: String,
    pub peer_addr: String,

    pub t_start_ms: u128,
    pub t_handshake_ok_ms: Option<u128>,
    pub t_end_ms: u128,

    pub alpn: Option<String>,
    pub http3: Http3Result,

    pub error: Option<String>,
    pub cfg: ConnectionConfig,
}

/// Pretty labels for logs
pub fn family_label(f: IpVersion) -> &'static str {
    match f {
        IpVersion::Auto => "Auto",
        IpVersion::Ipv4 => "IPv4",
        IpVersion::Ipv6 => "IPv6",
    }
}

#[derive(serde::Serialize)]
pub struct MetaRecord {
    pub host: String,
    pub peer_addr: SocketAddr,
    pub alpn: Option<String>,
    pub handshake_ok: bool,
    pub local_close: Option<String>,
    pub peer_close: Option<String>,
    pub enable_multipath: bool,
    pub stats: Option<BasicStats>,
}
#[derive(serde::Serialize)]
pub struct BasicStats {
    pub bytes_sent: u64,
    pub bytes_recv: u64,
    pub bytes_lost: u64,
    pub packets_sent: u64,
    pub packets_recv: u64,
    pub packets_lost: u64,
}
