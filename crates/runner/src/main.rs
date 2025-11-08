use anyhow::{anyhow, Result};
use core::config::{read_config, read_domains_iter};
use core::recorder::Recorder;
use core::throttle::RateLimit;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

fn main() -> Result<()> {
    // CLI: runner [config.toml]
    let cfg_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "in/config.toml".into());
    let cfg = read_config(&cfg_path)?;

    // Logging
    env_logger::builder()
        .filter_level(cfg.general.log_level)
        .init();

    // Load domains
    let domains_path = PathBuf::from(&cfg.io.in_dir).join(&cfg.io.domains_file_name);
    let domains: Vec<String> = read_domains_iter(&domains_path)?.collect();
    if domains.is_empty() {
        return Err(anyhow!("no domains found in {}", domains_path.display()));
    }

    // Recorder (one file per trace_id), colocated with qlog
    let recorder = Recorder::new(&cfg.io.out_dir)?;

    // Thread pool sizing
    let threads = if cfg.scheduler.concurrency == 0 {
        std::thread::available_parallelism()
            .map(|nz| nz.get())
            .unwrap_or(4)
    } else {
        cfg.scheduler.concurrency
    };

    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build_global()?;

    // Global rate limiter
    let rl = RateLimit::per_second(cfg.scheduler.requests_per_second, cfg.scheduler.burst);

    // Progress bar
    let total = domains.len() as u64;
    let start = Instant::now();
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} {pos}/{len} [{bar:40.cyan/blue}] \
             {percent}% | {elapsed_precise} < {eta_precise} | {per_sec} it/s | {msg}",
        )
        .unwrap(),
    );
    let pb = Arc::new(pb);
    let err_cnt = Arc::new(AtomicU64::new(0));

    domains.par_iter().for_each(|host| {
        if let Err(e) = probes::h3::probe(
            host,
            &cfg.io,
            &cfg.connection_config,
            &cfg.delay,
            &rl,
            &recorder.clone(),
        ) {
            err_cnt.fetch_add(1, Ordering::Relaxed);
            // avoid mangling the bar when printing errors
            pb.suspend(|| eprintln!("[{}] ERROR: {e:#}", host));
            let errs = err_cnt.load(Ordering::Relaxed);
            pb.set_message(format!("errors: {errs}"));
        }
        pb.inc(1);
    });

    pb.finish_with_message(format!(
        "done in {:.2}s, errors: {}",
        start.elapsed().as_secs_f32(),
        err_cnt.load(Ordering::Relaxed)
    ));

    Ok(())
}
