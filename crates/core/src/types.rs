use serde::Deserialize;

/// Which IP family to use when probing (config values: "auto", "ipv4", "ipv6", "both").
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IpVersion {
    Auto,
    Ipv4,
    Ipv6,
    Both,
}

impl Default for IpVersion {
    fn default() -> Self {
        IpVersion::Both
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ProbeOutcome {
    /// true if failure looked like timeout/ICMP "no QUIC here", so trying the other family makes sense
    pub retryable: bool,
}

impl ProbeOutcome {
    pub fn success() -> Self {
        Self { retryable: false }
    }
    pub fn retryable_fail() -> Self {
        Self { retryable: true }
    }
    pub fn nonretryable_fail() -> Self {
        Self { retryable: false }
    }
}

/// Pretty labels for logs
pub fn family_label(f: IpVersion) -> &'static str {
    match f {
        IpVersion::Auto => "Auto",
        IpVersion::Ipv4 => "IPv4",
        IpVersion::Ipv6 => "IPv6",
        IpVersion::Both => "Both",
    }
}
