use anyhow::{Context, Result};
use serde::Deserialize;
use std::{fs, io::{self, BufRead}, path::Path};

use crate::types::IpVersion;

#[derive(Debug, Clone, Deserialize)]
pub struct RootConfig {
    /// Probe attempt configurations (tried in order until one succeeds).
    #[serde(default)]
    pub attempts: Vec<ConnectionConfigConfig>,

    /// Scheduler knobs (threads, RPS, burst)
    #[serde(default)]
    pub scheduler: SchedulerConfig,

    /// Ethical pacing between attempts to the same domain
    #[serde(default)]
    pub delay: DelayConfig,

    /// Recorder / output knobs
    #[serde(default)]
    pub recorder: RecorderConfig,
}

// ---------------- Scheduler ----------------

#[derive(Debug, Clone, Deserialize)]
pub struct SchedulerConfig {
    /// Number of worker threads (0 = auto = CPU count)
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
    /// Global maximum "requests per second" (0 = unlimited)
    #[serde(default = "default_rps")]
    pub requests_per_second: u32,
    /// Short-term burst allowance for the limiter (tokens)
    #[serde(default = "default_burst")]
    pub burst: u32,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            concurrency: default_concurrency(),
            requests_per_second: default_rps(),
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
        Self { inter_attempt_delay_ms: default_inter_attempt_delay_ms() }
    }
}

// ---------------- Recorder ----------------

#[derive(Debug, Clone, Deserialize)]
pub struct RecorderConfig {
    /// Output directory; created if missing
    #[serde(default = "default_out_dir")]
    pub out_dir: String,
    /// File prefix (files are sharded as {prefix}-{shard:03}.jsonl)
    #[serde(default = "default_results_file_name")]
    pub results_file_name: String,
    /// Number of shard files to write in parallel (1..=1024)
    #[serde(default = "default_num_shards")]
    pub num_shards: usize,
}

impl Default for RecorderConfig {
    fn default() -> Self {
        Self {
            out_dir: default_out_dir(),
            results_file_name: default_results_file_name(),
            num_shards: default_num_shards(),
        }
    }
}

// ---------------- Attempt (QUIC/H3) ----------------

#[derive(Debug, Clone, Deserialize)]
pub struct ConnectionConfigConfig {
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

    // Timeouts (ms)
    #[serde(default = "default_handshake_timeout_ms")]
    pub handshake_timeout_ms: u64,
    #[serde(default = "default_overall_timeout_ms")]
    pub overall_timeout_ms: u64,
    #[serde(default = "default_max_idle_timeout_ms")]
    pub max_idle_timeout_ms: u64,

    // QUIC transport params
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

    // ALPN to advertise (e.g., ["h3"])
    #[serde(default = "default_alpn")]
    pub alpn: Vec<String>,

    // Preferred IP version for this connection config
    #[serde(default)]
    pub ip_version: IpVersion,

    // tquic multipath flags
    #[serde(default)]
    pub enable_multipath: bool,
    /// One of: "minrtt", "ecmp". Defaults to "minrtt".
    #[serde(default = "default_multipath_alg")]
    pub multipath_algorithm: String,
}

impl Default for ConnectionConfigConfig {
    fn default() -> Self {
        Self {
            port: default_port(),
            path: default_path(),
            user_agent: default_user_agent(),
            verify_peer: default_verify_peer(),
            ip_version: IpVersion::Auto,
            handshake_timeout_ms: default_handshake_timeout_ms(),
            overall_timeout_ms: default_overall_timeout_ms(),
            max_idle_timeout_ms: default_max_idle_timeout_ms(),
            initial_max_data: default_initial_max_data(),
            initial_max_stream_data_bidi_local: default_initial_max_data(),
            initial_max_stream_data_bidi_remote: default_initial_max_data(),
            initial_max_stream_data_uni: default_initial_max_data(),
            initial_max_streams_bidi: default_streams(),
            initial_max_streams_uni: default_streams(),
            alpn: default_alpn(),
            enable_multipath: false,
            multipath_algorithm: default_multipath_alg(),
        }
    }
}

// ---- defaults ----
fn default_concurrency() -> usize { 0 }   // 0 = auto
fn default_rps() -> u32 { 10 }
fn default_burst() -> u32 { 10 }
fn default_inter_attempt_delay_ms() -> u64 { 250 } // gentle default

fn default_port() -> u16 { 443 }
fn default_path() -> String { "/".into() }
fn default_user_agent() -> String { "User-Agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:143.0) Gecko/20100101 Firefox/143.0".into() }
fn default_verify_peer() -> bool { true }
fn default_handshake_timeout_ms() -> u64 { 4000 }
fn default_overall_timeout_ms() -> u64 { 8000 }
fn default_max_idle_timeout_ms() -> u64 { 5000 }
fn default_initial_max_data() -> u64 { 1_000_000 }
fn default_streams() -> u64 { 16 }
fn default_alpn() -> Vec<String> { vec!["h3".into()] }
fn default_multipath_alg() -> String { "minrtt".into() }

fn default_out_dir() -> String { "out".into() }
fn default_results_file_name() -> String { "results".into() }
fn default_num_shards() -> usize { 8 }

// ---- public API ----

pub fn read_config<P: AsRef<Path>>(p: P) -> Result<RootConfig> {
    let s = fs::read_to_string(&p)
        .with_context(|| format!("reading config file {}", p.as_ref().display()))?;
    let mut root: RootConfig = toml::from_str(&s)
        .with_context(|| format!("parsing TOML config {}", p.as_ref().display()))?;
    if root.attempts.is_empty() {
        // ensure at least one default attempt
        root.attempts.push(ConnectionConfigConfig::default());
    }
    if root.recorder.num_shards == 0 || root.recorder.num_shards > 1024 {
        root.recorder.num_shards = default_num_shards();
    }
    Ok(root)
}

/// Stream domains lazily from a file. Lines may contain comments starting with '#'.
pub fn read_domains_iter<P: AsRef<Path>>(p: P) -> Result<impl Iterator<Item = String>> {
    let file = fs::File::open(&p)
        .with_context(|| format!("opening domains list {}", p.as_ref().display()))?;
    let reader = io::BufReader::new(file);
    // We return an iterator that owns the reader via into_lines().
    Ok(reader
        .lines()
        .filter_map(|l| l.ok())
        .filter_map(|line| {
            let trimmed = line.split('#').next().unwrap_or("").trim().to_string();
            if trimmed.is_empty() { None } else { Some(trimmed) }
        }))
}
