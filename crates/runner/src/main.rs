use anyhow::{anyhow, Result};
use core::config::{effective_config, read_config, read_domains};
use core::logging::init_default_logging;
use core::throttle::RateLimit;
use probes::h3;
use rayon::prelude::*;

fn main() -> Result<()> {
    init_default_logging();

    // CLI: runner [config.toml] [domains.txt]
    let mut args = std::env::args().skip(1);
    let cfg_path = args.next().unwrap_or_else(|| "config.toml".into());
    let domains_path = args.next().unwrap_or_else(|| "domains.txt".into());

    let root = read_config(&cfg_path)?;
    let domains = read_domains(&domains_path)?;
    if domains.is_empty() {
        return Err(anyhow!("no domains found in {}", domains_path));
    }

    // Prepare scheduler knobs
    let mut threads = root.scheduler.concurrency;
    if threads == 0 {
        // Auto: number of logical CPUs
        threads = std::thread::available_parallelism()
            .map(|nz| nz.get())
            .unwrap_or(4);
    }

    // Build global thread pool
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build_global()
        .expect("failed to init global rayon thread pool");

    // Global, shared rate-limiter
    let rl = RateLimit::per_second(
        root.scheduler.requests_per_second,
        root.scheduler.burst,
    );

    // Parallel over domains
    domains
        .par_iter()
        .for_each(|host| {
            let eff = effective_config(&root, host);
            if let Err(e) = h3::probe(host, &eff, &rl) {
                eprintln!("[{}] ERROR: {e:#}", host);
            }
            println!();
        });

    Ok(())
}
