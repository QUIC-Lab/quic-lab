use anyhow::{anyhow, Result};
use core::config::{read_config, read_domains_iter};
use core::qlog;
use core::recorder::Recorder;
use core::throttle::RateLimit;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use rayon::prelude::*;
use std::io::{stderr, stdout, IsTerminal};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

// TTY-recognition
fn is_tty() -> bool {
    stdout().is_terminal() || stderr().is_terminal()
}

fn fmt_hms(mut secs: u64) -> String {
    let h = secs / 3600;
    secs %= 3600;
    let m = secs / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

fn main() -> Result<()> {
    // CLI: runner [config.toml]
    let cfg_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "in/config.toml".into());
    let cfg = read_config(&cfg_path)?;

    // Logging
    let _run_log = core::logging::init_file_logger(&cfg.io.out_dir, cfg.general.log_level)?;

    // QLOG sink (flat folder + rotation)
    qlog::init(&cfg.io.out_dir, cfg.general.save_qlog_files)?;

    // Load domains
    let domains_path = PathBuf::from(&cfg.io.in_dir).join(&cfg.io.domains_file_name);
    let domains: Vec<String> = read_domains_iter(&domains_path)?.collect();
    if domains.is_empty() {
        return Err(anyhow!("no domains found in {}", domains_path.display()));
    }

    // Recorder (one file per trace_id)
    let recorder = Recorder::new(&cfg.io.out_dir, cfg.general.save_recorder_files)?;

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
    let processed = Arc::new(AtomicU64::new(0));
    let err_cnt = Arc::new(AtomicU64::new(0));

    let use_tty = is_tty();

    // Reporter-Thread for Non-TTY
    let done_flag = Arc::new(AtomicBool::new(false));
    let reporter = if !use_tty {
        let processed_c = processed.clone();
        let err_c = err_cnt.clone();
        let done_c = done_flag.clone();
        Some(std::thread::spawn(move || {
            // Every 10 seconds
            while !done_c.load(Ordering::Relaxed) {
                let p = processed_c.load(Ordering::Relaxed);
                let e = err_c.load(Ordering::Relaxed);
                let elapsed = start.elapsed().as_secs_f64();
                let rate = if elapsed > 0.0 {
                    p as f64 / elapsed
                } else {
                    0.0
                };
                let remain = total.saturating_sub(p);
                let eta = if rate > 0.0 {
                    (remain as f64 / rate) as u64
                } else {
                    0
                };
                eprintln!(
                    "[progress] {}/{} done | {} elapsed | ETA {} | {:.1} it/s | errors: {}",
                    p,
                    total,
                    fmt_hms(start.elapsed().as_secs()),
                    fmt_hms(eta),
                    rate,
                    e
                );
                std::thread::sleep(Duration::from_secs(10));
            }
            // Finish message
            let p = processed_c.load(Ordering::Relaxed);
            let e = err_c.load(Ordering::Relaxed);
            eprintln!(
                "[progress] done {}/{} in {} | errors: {}",
                p,
                total,
                fmt_hms(start.elapsed().as_secs()),
                e
            );
        }))
    } else {
        None
    };

    // TTY-Progressbar setup
    let pb = if use_tty {
        let pb = ProgressBar::new(total);
        pb.set_draw_target(ProgressDrawTarget::stderr());
        pb.set_style(ProgressStyle::with_template(
            "{spinner:.green} {pos}/{len} [{bar:40.cyan/blue}] \
                 {percent}% | {elapsed_precise} < {eta_precise} | {per_sec} it/s | {msg}",
        )?);
        pb.enable_steady_tick(Duration::from_millis(80));
        Some(Arc::new(pb))
    } else {
        None
    };

    domains.par_iter().for_each(|host| {
        if let Err(e) = probes::h3::probe(
            host,
            &cfg.scheduler,
            &cfg.io,
            &cfg.general,
            &cfg.connection_config,
            &rl,
            &recorder,
        ) {
            err_cnt.fetch_add(1, Ordering::Relaxed);
            log::error!("[{}] ERROR: {e:#}", host);
            if let Some(pb) = &pb {
                let errs = err_cnt.load(Ordering::Relaxed);
                pb.set_message(format!("errors: {errs}"));
            }
        }
        processed.fetch_add(1, Ordering::Relaxed);
        if let Some(pb) = &pb {
            pb.inc(1);
        }
    });

    if let Some(pb) = &pb {
        pb.finish_with_message(format!(
            "done in {:.2}s, errors: {}",
            start.elapsed().as_secs_f32(),
            err_cnt.load(Ordering::Relaxed)
        ));
    }

    // Cancel Reporter-Thread, if non-TTY
    if reporter.is_some() {
        done_flag.store(true, Ordering::Relaxed);
        let _ = reporter.unwrap().join();
    }

    Ok(())
}
