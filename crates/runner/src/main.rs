use anyhow::{anyhow, Result};
use core::config::{effective_config, read_config, read_domains};
use core::logging::init_default_logging;
use probes::h3;

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

    for host in domains {
        let eff = effective_config(&root, &host);
        if let Err(e) = h3::probe(&host, &eff) {
            eprintln!("[{}] ERROR: {e:#}", host);
        }
        println!();
    }

    Ok(())
}
