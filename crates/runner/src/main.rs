use anyhow::{anyhow, Result};
use core::config::read_config;
use core::logging::init_default_logging;
use core::recorder::Recorder;
use core::throttle::RateLimit;
use rayon::prelude::*;
use std::fs;
use std::io::{self, BufRead};
use std::path::{Path, PathBuf};

/// Local copy of the previous read_domains (trims, ignores comments/blank lines).
fn read_domains<P: AsRef<Path>>(p: P) -> Result<Vec<String>> {
    let file = fs::File::open(&p)
        .map_err(|e| anyhow!("opening domains list {}: {e}", p.as_ref().display()))?;
    let reader = io::BufReader::new(file);
    let mut out = Vec::new();
    for line in reader.lines() {
        let line = line?;
        let trimmed = line.split('#').next().unwrap_or("").trim();
        if !trimmed.is_empty() {
            out.push(trimmed.to_string());
        }
    }
    Ok(out)
}

/// Convert connection config -> transport/recording ConnectionConfigConfig.
fn to_types_attempt(att: &core::config::ConnectionConfigConfig) -> core::types::ConnectionConfigConfig {
    core::types::ConnectionConfigConfig {
        // network + family
        port: att.port,
        ip_version: att.ip_version,

        // HTTP request
        path: att.path.clone(),
        user_agent: att.user_agent.clone(),

        // TLS/ALPN
        alpn: att.alpn.clone(),
        verify_peer: att.verify_peer,

        // timeouts
        max_idle_timeout_ms: att.max_idle_timeout_ms,
        handshake_timeout_ms: att.handshake_timeout_ms,
        overall_timeout_ms: att.overall_timeout_ms,

        // flow control
        initial_max_data: att.initial_max_data,
        initial_max_stream_data_bidi_local: att.initial_max_stream_data_bidi_local,
        initial_max_stream_data_bidi_remote: att.initial_max_stream_data_bidi_remote,
        initial_max_stream_data_uni: att.initial_max_stream_data_uni,
        initial_max_streams_bidi: att.initial_max_streams_bidi,
        initial_max_streams_uni: att.initial_max_streams_uni,

        // multipath
        multipath: att.enable_multipath,
        multipath_algorithm: Option::from(att.multipath_algorithm.clone()),
    }
}

fn main() -> Result<()> {
    init_default_logging();

    // CLI: runner [config.toml] [domains.txt]
    let mut args = std::env::args().skip(1);
    let cfg_path = args.next().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "in/config.toml".into());
    let domains_path = args.next().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "in/domains.txt".into());

    let config_root = read_config(&cfg_path)?;
    let domains = read_domains(&domains_path)?;
    if domains.is_empty() {
        return Err(anyhow!("no domains found in {}", domains_path));
    }

    // JSONL recorder (change path if you have it in config)
    let mut path = PathBuf::from(&config_root.recorder.out_dir);
    path.push(&config_root.recorder.results_file_name);
    path.set_extension("jsonl");
    // Make sure the parent directory exists
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?; // create all directories if missing
    }
    let recorder = Recorder::open(path)?;

    // Scheduler knobs
    let mut threads = config_root.scheduler.concurrency;
    if threads == 0 {
        threads = std::thread::available_parallelism()
            .map(|nz| nz.get())
            .unwrap_or(4);
    }

    // Global thread pool
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build_global()
        .expect("failed to init global rayon thread pool");

    // Global, shared rate-limiter
    let rl = RateLimit::per_second(
        config_root.scheduler.requests_per_second,
        config_root.scheduler.burst,
    );

    // Convert attempts from config type -> transport type
    let attempts_typed: Vec<core::types::ConnectionConfigConfig> =
        config_root.connection_config.iter().map(to_types_attempt).collect();

    // Parallel over domains
    domains.par_iter().for_each(|host| {
        if let Err(e) = probes::h3::probe(
            host,
            &attempts_typed,   // &[core::types::ConnectionConfigConfig]
            &config_root.delay,      // your delay/cooldown struct
            &rl,
            &recorder,
        ) {
            eprintln!("[{}] ERROR: {e:#}", host);
        }
    });

    Ok(())
}
