use crate::types::IpVersion;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, BufRead},
    path::Path,
};

#[derive(Debug, Clone, Deserialize)]
pub struct RootConfig {
    /// Scheduler knobs (threads, RPS, burst)
    #[serde(default)]
    pub general: GeneralConfig,

    /// Scheduler knobs (threads, RPS, burst)
    #[serde(default)]
    pub scheduler: SchedulerConfig,

    /// Ethical pacing between attempts to the same domain
    #[serde(default)]
    pub delay: DelayConfig,

    /// Recorder / output knobs
    #[serde(default)]
    pub io: IOConfig,

    /// Probe attempt configurations (tried in order until one succeeds).
    #[serde(default)]
    pub connection_config: Vec<ConnectionConfig>,
}

// ---------------- Scheduler ----------------

#[derive(Debug, Clone, Deserialize)]
pub struct SchedulerConfig {
    /// Number of worker threads (0 = auto = CPU count)
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
    /// Global maximum "requests per second" (0 = unlimited)
    #[serde(default = "default_requests_per_second")]
    pub requests_per_second: u32,
    /// Short-term burst allowance for the limiter (tokens)
    #[serde(default = "default_burst")]
    pub burst: u32,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            concurrency: default_concurrency(),
            requests_per_second: default_requests_per_second(),
            burst: default_burst(),
        }
    }
}

// ---------------- Delay ----------------

#[derive(Debug, Clone, Deserialize)]
pub struct DelayConfig {
    /// Delay between attempts to the same domain (milliseconds)
    #[serde(default = "default_inter_attempt_delay_ms")]
    pub inter_attempt_delay_ms: u64,
}

impl Default for DelayConfig {
    fn default() -> Self {
        Self {
            inter_attempt_delay_ms: default_inter_attempt_delay_ms(),
        }
    }
}

// ---------------- IO ----------------

#[derive(Debug, Clone, Deserialize)]
pub struct IOConfig {
    /// Input directory; created if missing
    #[serde(default = "default_in_dir")]
    pub in_dir: String,

    /// Filename of domain list (must be inside the input directory)
    #[serde(default = "default_domains_file_name")]
    pub domains_file_name: String,

    /// Output directory; created if missing
    #[serde(default = "default_out_dir")]
    pub out_dir: String,
}

impl Default for IOConfig {
    fn default() -> Self {
        Self {
            in_dir: default_in_dir(),
            domains_file_name: default_domains_file_name(),
            out_dir: default_out_dir(),
        }
    }
}

// ---------------- General ----------------

#[derive(Debug, Clone, Deserialize)]
pub struct GeneralConfig {
    /// Log level, support OFF/ERROR/WARN/INFO/DEBUG/TRACE.
    #[serde(default = "default_log_level")]
    pub log_level: log::LevelFilter,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            log_level: default_log_level(),
        }
    }
}

// ---------------- Attempt (QUIC/H3) ----------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionConfig {
    // Application-layer knobs
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_path")]
    pub path: String,
    #[serde(default = "default_user_agent")]
    pub user_agent: String,

    // TLS / verification
    #[serde(default = "default_verify_peer")]
    pub verify_peer: bool,

    // ALPN to advertise (e.g., ["h3"])
    #[serde(default = "default_alpn")]
    pub alpn: Vec<String>,

    // Preferred IP version for this connection config
    #[serde(default)]
    pub ip_version: IpVersion,

    // Timeouts (ms)
    #[serde(default = "default_max_idle_timeout_ms")]
    pub max_idle_timeout_ms: u64,

    // QUIC transport params
    #[serde(default = "default_initial_max_data")]
    pub initial_max_data: u64,
    #[serde(default = "default_initial_max_stream_data_bidi_local")]
    pub initial_max_stream_data_bidi_local: u64,
    #[serde(default = "default_initial_max_stream_data_bidi_remote")]
    pub initial_max_stream_data_bidi_remote: u64,
    #[serde(default = "default_initial_max_stream_data_uni")]
    pub initial_max_stream_data_uni: u64,
    #[serde(default = "default_initial_max_streams_bidi")]
    pub initial_max_streams_bidi: u64,
    #[serde(default = "default_initial_max_streams_uni")]
    pub initial_max_streams_uni: u64,
    #[serde(default = "default_max_ack_delay")]
    pub max_ack_delay: u64,
    #[serde(default = "default_active_connection_id_limit")]
    pub active_connection_id_limit: u64,
    #[serde(default = "default_send_udp_payload_size")]
    pub send_udp_payload_size: usize,
    #[serde(default = "default_max_receive_buffer_size")]
    pub max_receive_buffer_size: usize,

    // tquic multipath flags
    #[serde(default = "default_enable_multipath")]
    pub enable_multipath: bool,
    /// One of: "minrtt", "roundrobin", "redundant". Defaults to "minrtt".
    #[serde(default = "default_multipath_algorithm")]
    pub multipath_algorithm: String,
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            port: default_port(),
            path: default_path(),
            user_agent: default_user_agent(),
            verify_peer: default_verify_peer(),
            ip_version: IpVersion::Auto,
            alpn: default_alpn(),
            max_idle_timeout_ms: default_max_idle_timeout_ms(),
            initial_max_data: default_initial_max_data(),
            initial_max_stream_data_bidi_local: default_initial_max_stream_data_bidi_local(),
            initial_max_stream_data_bidi_remote: default_initial_max_stream_data_bidi_remote(),
            initial_max_stream_data_uni: default_initial_max_stream_data_uni(),
            initial_max_streams_bidi: default_initial_max_streams_bidi(),
            initial_max_streams_uni: default_initial_max_streams_uni(),
            max_ack_delay: default_max_ack_delay(),
            active_connection_id_limit: default_active_connection_id_limit(),
            send_udp_payload_size: default_send_udp_payload_size(),
            max_receive_buffer_size: default_max_receive_buffer_size(),
            enable_multipath: default_enable_multipath(),
            multipath_algorithm: default_multipath_algorithm(),
        }
    }
}

// ---- General defaults ----
fn default_concurrency() -> usize {
    0
} // 0 = auto
fn default_requests_per_second() -> u32 {
    200
}
fn default_burst() -> u32 {
    200
}
fn default_inter_attempt_delay_ms() -> u64 {
    200
}

// ---- Attempt defaults ----

fn default_port() -> u16 {
    443
}
fn default_path() -> String {
    "/".into()
}
fn default_user_agent() -> String {
    "QUIC-Lab (research; no-harm-intended; opt-out: [INSERT CONTACT INFO])".into()
}
fn default_verify_peer() -> bool {
    true
}
fn default_alpn() -> Vec<String> {
    vec!["h3".into()]
}
fn default_max_idle_timeout_ms() -> u64 {
    30000
}
fn default_initial_max_data() -> u64 {
    10_485_760
}
fn default_initial_max_stream_data_bidi_local() -> u64 {
    5_242_880
}
fn default_initial_max_stream_data_bidi_remote() -> u64 {
    2_097_152
}
fn default_initial_max_stream_data_uni() -> u64 {
    1_048_576
}
fn default_initial_max_streams_bidi() -> u64 {
    200
}
fn default_initial_max_streams_uni() -> u64 {
    100
}
fn default_max_ack_delay() -> u64 {
    25
}
fn default_active_connection_id_limit() -> u64 {
    2
}
fn default_send_udp_payload_size() -> usize {
    1200
}
fn default_max_receive_buffer_size() -> usize {
    65536
}
fn default_enable_multipath() -> bool {
    false
}
fn default_multipath_algorithm() -> String {
    "minrtt".into()
}

// ---- IO defaults ----
fn default_in_dir() -> String {
    "in".into()
}
fn default_domains_file_name() -> String {
    "domains.txt".into()
}
fn default_out_dir() -> String {
    "out".into()
}
fn default_log_level() -> log::LevelFilter {
    log::LevelFilter::Info
}

// ---- public API ----

pub fn read_config<P: AsRef<Path>>(p: P) -> Result<RootConfig> {
    let s = fs::read_to_string(&p)
        .with_context(|| format!("reading config file {}", p.as_ref().display()))?;
    let mut root: RootConfig = toml::from_str(&s)
        .with_context(|| format!("parsing TOML config {}", p.as_ref().display()))?;
    if root.connection_config.is_empty() {
        // ensure at least one default attempt
        root.connection_config.push(ConnectionConfig::default());
    }
    Ok(root)
}

/// Stream domains lazily from a file. Lines may contain comments starting with '#'.
pub fn read_domains_iter<P: AsRef<Path>>(p: P) -> Result<impl Iterator<Item = String>> {
    let file = fs::File::open(&p)
        .with_context(|| format!("opening domains list {}", p.as_ref().display()))?;
    let reader = io::BufReader::new(file);
    // We return an iterator that owns the reader via into_lines().
    Ok(reader.lines().filter_map(|l| l.ok()).filter_map(|line| {
        let trimmed = line.split('#').next().unwrap_or("").trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    }))
}
