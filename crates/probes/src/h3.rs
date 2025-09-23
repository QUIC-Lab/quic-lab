use anyhow::Result;

use core::config::{DelayConfig};
use core::types::ConnectionConfigConfig;
use core::recorder::Recorder;
use core::resolver::{resolve_peer, resolve_peers_for_both};
use core::throttle::RateLimit;
use core::transport::quic;
use core::types::{IpVersion};

/// Try a sequence of connection configs; stop at first success. Every connection config is recorded.
pub fn probe(
    host: &str,
    connection_configs: &[ConnectionConfigConfig],
    delay: &DelayConfig,
    rl: &RateLimit,
    recorder: &Recorder,
) -> Result<()> {
    // Go through connection config list
    for (idx, att) in connection_configs.iter().enumerate() {
        let fam = att.ip_version;
        let mut attempt_succeeded = false;

        // Resolve per attempt/family
        let targets: Vec<(IpVersion, std::net::SocketAddr)> = match fam {
            IpVersion::Both => {
                let (v4, v6) = resolve_peers_for_both(host, att.port)?;
                let mut v = Vec::with_capacity(2);
                if let Some(a) = v4 { v.push((IpVersion::Ipv4, a)); }
                if let Some(a) = v6 { v.push((IpVersion::Ipv6, a)); }
                v
            }
            IpVersion::Auto | IpVersion::Ipv4 | IpVersion::Ipv6 => {
                let a = resolve_peer(host, att.port, fam)?;
                vec![(if a.is_ipv4() { IpVersion::Ipv4 } else { IpVersion::Ipv6 }, a)]
            }
        };

        for (fam_eff, addr) in targets {
            rl.until_ready();

            let (rec, outcome) = quic::run(host, addr, fam_eff, att)?;
            recorder.write(&rec);

            if !outcome.retryable {
                attempt_succeeded = true;
                break;
            }
        }

        // Stop at first successful attempt
        if attempt_succeeded {
            break;
        } else if idx + 1 < connection_configs.len() && delay.inter_attempt_delay_ms > 0 {
            std::thread::sleep(std::time::Duration::from_millis(delay.inter_attempt_delay_ms));
        }
    }

    Ok(())
}
