use serde::{Deserialize, Serialize};

/// Which IP family to use when probing (config values: "auto", "ipv4", "ipv6", "both").
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IpVersion {
    Auto,
    Ipv4,
    Ipv6,
    Both,
}

impl Default for IpVersion {
    fn default() -> Self {
        IpVersion::Both
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ProbeOutcome {
    /// true if failure looked like timeout/ICMP "no QUIC here", so trying the other family makes sense
    pub retryable: bool,
}

impl ProbeOutcome {
    pub fn success() -> Self { Self { retryable: false } }
    pub fn retryable_fail() -> Self { Self { retryable: true } }
    pub fn nonretryable_fail() -> Self { Self { retryable: false } }
}

/// Compress the connection config we store per-record (keeps lines small).
#[derive(Debug, Clone, Serialize)]
pub struct MinimalConnectionConfigCfg {
    pub alpn: Vec<String>,
    pub verify_peer: bool,
    pub multipath: bool,
    pub multipath_algorithm: Option<String>,
}

/// Full attempt config as consumed by the transport (deserializable from TOML if you wire it).
#[derive(Debug, Clone, Deserialize)]
pub struct ConnectionConfigConfig {
    // New fields so probes can resolve address and choose family
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_ip_version")]
    pub ip_version: IpVersion,

    // HTTP request bits
    #[serde(default = "default_path")]
    pub path: String,
    #[serde(default = "user_agent")]
    pub user_agent: String,

    // ALPN + TLS
    #[serde(default = "default_alpn")]
    pub alpn: Vec<String>,
    #[serde(default = "default_verify_peer")]
    pub verify_peer: bool,

    // transport params / timeouts (millis)
    #[serde(default = "default_max_idle_timeout_ms")]
    pub max_idle_timeout_ms: u64,
    #[serde(default = "default_handshake_timeout_ms")]
    pub handshake_timeout_ms: u64,
    #[serde(default = "default_overall_timeout_ms")]
    pub overall_timeout_ms: u64,

    // flow-control
    #[serde(default = "default_initial_max_data")]
    pub initial_max_data: u64,
    #[serde(default = "default_initial_max_data")]
    pub initial_max_stream_data_bidi_local: u64,
    #[serde(default = "default_initial_max_data")]
    pub initial_max_stream_data_bidi_remote: u64,
    #[serde(default = "default_initial_max_data")]
    pub initial_max_stream_data_uni: u64,
    #[serde(default = "default_streams")]
    pub initial_max_streams_bidi: u64,
    #[serde(default = "default_streams")]
    pub initial_max_streams_uni: u64,

    // multipath
    #[serde(default)]
    pub multipath: bool,
    #[serde(default)]
    pub multipath_algorithm: Option<String>,
}

fn default_alpn() -> Vec<String> { vec!["h3".into()] }
fn default_verify_peer() -> bool { true }
fn default_path() -> String { "/".into() }
fn user_agent() -> String { "User-Agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:143.0) Gecko/20100101 Firefox/143.0".into() }
fn default_port() -> u16 { 443 }
fn default_ip_version() -> IpVersion { IpVersion::Both }

fn default_handshake_timeout_ms() -> u64 { 4000 }
fn default_overall_timeout_ms() -> u64 { 8000 }
fn default_max_idle_timeout_ms() -> u64 { 5000 }
fn default_initial_max_data() -> u64 { 1_000_000 }
fn default_streams() -> u64 { 16 }

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

    pub cfg: MinimalConnectionConfigCfg,
}

/// Pretty labels for logs
pub fn family_label(f: IpVersion) -> &'static str {
    match f {
        IpVersion::Auto => "Auto",
        IpVersion::Ipv4 => "IPv4",
        IpVersion::Ipv6 => "IPv6",
        IpVersion::Both => "Both",
    }
}
