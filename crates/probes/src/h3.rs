use anyhow::Result;

use core::config::DomainConfig;
use core::resolver::{resolve_peer, resolve_peers_for_both};
use core::throttle::RateLimit;
use core::transport::quic::run;
use core::types::{family_label, IpVersion};

/// Minimal H3 GET probe.
/// Handles Auto / IPv4 / IPv6 / Both by calling into core resolver + transport.
pub fn probe(host: &str, cfg: &DomainConfig, rl: &RateLimit) -> Result<()> {
    match cfg.ip_version {
        IpVersion::Both => {
            let (v4, v6) = resolve_peers_for_both(host, cfg.port)?;
            if let Some(addr) = v4 {
                println!(
                    "   [{}] Connecting to {} (IPv4)",
                    family_label(IpVersion::Ipv4),
                    addr
                );
                rl.until_ready();               // <- throttle
                if let Err(e) = run(host, addr, cfg) {
                    eprintln!("   [{}] {}", family_label(IpVersion::Ipv4), e);
                }
            } else {
                eprintln!(
                    "   [{}] Skipping: no IPv4 address",
                    family_label(IpVersion::Ipv4)
                );
            }

            if let Some(addr) = v6 {
                println!(
                    "   [{}] Connecting to {} (IPv6)",
                    family_label(IpVersion::Ipv6),
                    addr
                );
                rl.until_ready();               // <- throttle
                if let Err(e) = run(host, addr, cfg) {
                    eprintln!("   [{}] {}", family_label(IpVersion::Ipv6), e);
                }
            } else {
                eprintln!(
                    "   [{}] Skipping: no IPv6 address",
                    family_label(IpVersion::Ipv6)
                );
            }
            Ok(())
        }

        IpVersion::Auto => {
            // Try OS-preferred first
            let first = resolve_peer(host, cfg.port, IpVersion::Auto)?;
            let first_is_v4 = first.is_ipv4();
            println!(
                "   [Auto] Connecting to {} ({})",
                first,
                if first_is_v4 { "IPv4" } else { "IPv6" }
            );
            rl.until_ready();                   // <- throttle
            match run(host, first, cfg) {
                Ok(outcome) if outcome.retryable => {
                    // Flip family and try once
                    let alt_family = if first_is_v4 { IpVersion::Ipv6 } else { IpVersion::Ipv4 };
                    if let Ok(alt) = resolve_peer(host, cfg.port, alt_family) {
                        println!(
                            "   [Auto→fallback] Connecting to {} ({})",
                            alt,
                            if alt.is_ipv4() { "IPv4" } else { "IPv6" }
                        );
                        rl.until_ready();       // <- throttle
                        let _ = run(host, alt, cfg);
                    }
                    Ok(())
                }
                Ok(_) => Ok(()),
                Err(e) => Err(e),
            }
        }

        fam @ (IpVersion::Ipv4 | IpVersion::Ipv6) => {
            let addr = resolve_peer(host, cfg.port, fam)?;
            println!(
                "   [{}] Connecting to {} ({})",
                family_label(fam),
                addr,
                if addr.is_ipv4() { "IPv4" } else { "IPv6" }
            );
            rl.until_ready();                   // <- throttle
            let _ = run(host, addr, cfg)?;
            Ok(())
        }
    }
}
