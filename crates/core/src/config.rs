use anyhow::{Context, Result};
use serde::Deserialize;
use std::{collections::HashMap, fs, io::{self, BufRead}, path::Path};

use crate::types::IpVersion;

#[derive(Debug, Clone, Deserialize)]
pub struct RootConfig {
    #[serde(default)]
    pub global: DomainConfig,
    #[serde(default)]
    pub domains: HashMap<String, DomainConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DomainConfig {
    // High-level probe knobs
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_path")]
    pub path: String,
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

    // Preferred IP version for this domain
    #[serde(default)]
    pub ip_version: IpVersion,
}

impl Default for DomainConfig {
    fn default() -> Self {
        Self {
            port: default_port(),
            path: default_path(),
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

        }
    }
}

// ---- defaults ----

fn default_port() -> u16 { 443 }
fn default_path() -> String { "/".into() }
fn default_verify_peer() -> bool { true }
fn default_handshake_timeout_ms() -> u64 { 4000 }
fn default_overall_timeout_ms() -> u64 { 8000 }
fn default_max_idle_timeout_ms() -> u64 { 5000 }
fn default_initial_max_data() -> u64 { 1_000_000 }
fn default_streams() -> u64 { 16 }
fn default_alpn() -> Vec<String> { vec!["h3".into()] }

// ---- public API ----

pub fn read_config<P: AsRef<Path>>(p: P) -> Result<RootConfig> {
    let s = fs::read_to_string(&p)
        .with_context(|| format!("reading config file {}", p.as_ref().display()))?;
    let mut root: RootConfig = toml::from_str(&s)
        .with_context(|| format!("parsing TOML config {}", p.as_ref().display()))?;
    // Ensure default global if omitted
    if root.global.alpn.is_empty() {
        root.global.alpn = default_alpn();
    }
    Ok(root)
}

pub fn read_domains<P: AsRef<Path>>(p: P) -> Result<Vec<String>> {
    let file = fs::File::open(&p)
        .with_context(|| format!("opening domains list {}", p.as_ref().display()))?;
    let reader = io::BufReader::new(file);
    let mut out = Vec::new();
    for line in reader.lines() {
        let line = line?;
        // Support inline comments: "example.com   # stuff"
        let trimmed = line.split('#').next().unwrap_or("").trim();
        if trimmed.is_empty() {
            continue;
        }
        out.push(trimmed.to_string());
    }
    Ok(out)
}

pub fn effective_config(root: &RootConfig, host: &str) -> DomainConfig {
    let mut eff = root.global.clone();
    if let Some(dom) = root.domains.get(host) {
        eff.port = dom.port;
        eff.path = dom.path.clone();
        eff.verify_peer = dom.verify_peer;
        eff.handshake_timeout_ms = dom.handshake_timeout_ms;
        eff.overall_timeout_ms = dom.overall_timeout_ms;
        eff.max_idle_timeout_ms = dom.max_idle_timeout_ms;
        eff.initial_max_data = dom.initial_max_data;
        eff.initial_max_stream_data_bidi_local = dom.initial_max_stream_data_bidi_local;
        eff.initial_max_stream_data_bidi_remote = dom.initial_max_stream_data_bidi_remote;
        eff.initial_max_stream_data_uni = dom.initial_max_stream_data_uni;
        eff.initial_max_streams_bidi = dom.initial_max_streams_bidi;
        eff.initial_max_streams_uni = dom.initial_max_streams_uni;
        eff.alpn = dom.alpn.clone();
        eff.ip_version = dom.ip_version;
    }
    eff
}
