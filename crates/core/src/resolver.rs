use anyhow::{anyhow, Result};
use std::net::{SocketAddr, ToSocketAddrs};

use crate::types::IpVersion;

/// Resolve a single address honoring an explicit family
pub fn resolve_peer(host: &str, port: u16, family: IpVersion) -> Result<SocketAddr> {
    let addrs = (host, port).to_socket_addrs()?;
    let pick = match family {
        IpVersion::Auto => addrs.into_iter().next(),
        IpVersion::Ipv4 => addrs.into_iter().find(|a| a.is_ipv4()),
        IpVersion::Ipv6 => addrs.into_iter().find(|a| a.is_ipv6()),
        IpVersion::Both => unreachable!("use resolve_peers_for_both() for Both"),
    };
    pick.ok_or_else(|| anyhow!("no matching address for {host}:{port} ({:?})", family))
}

/// Resolve one IPv4 and/or one IPv6 when Both is requested
pub fn resolve_peers_for_both(
    host: &str,
    port: u16,
) -> Result<(Option<SocketAddr>, Option<SocketAddr>)> {
    let mut v4: Option<SocketAddr> = None;
    let mut v6: Option<SocketAddr> = None;

    for addr in (host, port).to_socket_addrs()? {
        if addr.is_ipv4() && v4.is_none() {
            v4 = Some(addr);
        }
        if addr.is_ipv6() && v6.is_none() {
            v6 = Some(addr);
        }
        if v4.is_some() && v6.is_some() {
            break;
        }
    }

    if v4.is_none() && v6.is_none() {
        return Err(anyhow!("no A/AAAA addresses for {host}:{port}"));
    }
    Ok((v4, v6))
}

/// Resolve per-attempt targets based on IpVersion choice.
pub fn resolve_targets(
    host: &str,
    port: u16,
    family: IpVersion,
) -> Result<Vec<(IpVersion, SocketAddr)>> {
    match family {
        IpVersion::Both => {
            let (v4, v6) = resolve_peers_for_both(host, port)?;
            let mut out = Vec::with_capacity(2);
            if let Some(a) = v4 {
                out.push((IpVersion::Ipv4, a));
            }
            if let Some(a) = v6 {
                out.push((IpVersion::Ipv6, a));
            }
            Ok(out)
        }
        IpVersion::Auto | IpVersion::Ipv4 | IpVersion::Ipv6 => {
            let a = resolve_peer(host, port, family)?;
            let fam = if a.is_ipv4() {
                IpVersion::Ipv4
            } else {
                IpVersion::Ipv6
            };
            Ok(vec![(fam, a)])
        }
    }
}
