//! SSRF guardrail: reject requests to hosts that resolve to non-public IPs
//! (loopback, RFC1918 private ranges, link-local, cloud metadata endpoint).
//!
//! This range list is a deliberate security decision left to the plugin
//! author — scaffolded here with signature + failing tests only.

use std::collections::HashSet;
use std::net::IpAddr;

/// Env var holding an operator-supplied extra blocklist, on top of the
/// built-in ranges: comma-separated entries, each either a literal IP
/// address (blocks that address) or a bare hostname (blocks that hostname,
/// compared case-insensitively against the request's host before DNS
/// resolution — covers hosts an operator wants to deny by name even if
/// their IP isn't otherwise private, e.g. an internal DNS alias that
/// resolves to a public-looking IP via split-horizon DNS).
pub const EXTRA_BLOCKLIST_ENV: &str = "NETWORK_PLUGIN_EXTRA_BLOCKED_HOSTS";

/// Operator-configurable extension of the built-in SSRF blocklist. Built
/// once at plugin startup from [`EXTRA_BLOCKLIST_ENV`] and shared (via
/// `Arc`) with the resolver used for every connection.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Blocklist {
    extra_ips: HashSet<IpAddr>,
    extra_hosts: HashSet<String>,
}

impl Blocklist {
    /// Parse a comma-separated list of IPs and/or hostnames. Empty/blank
    /// entries are ignored; unparseable-as-IP entries are treated as
    /// hostnames as-is (lowercased for comparison).
    pub fn parse(raw: &str) -> Self {
        let mut extra_ips = HashSet::new();
        let mut extra_hosts = HashSet::new();
        for entry in raw.split(',') {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            match entry.parse::<IpAddr>() {
                Ok(ip) => {
                    extra_ips.insert(ip);
                }
                Err(_) => {
                    extra_hosts.insert(entry.to_lowercase());
                }
            }
        }
        Self { extra_ips, extra_hosts }
    }

    /// Load from [`EXTRA_BLOCKLIST_ENV`]; empty (blocks nothing extra) if
    /// unset.
    pub fn from_env() -> Self {
        std::env::var(EXTRA_BLOCKLIST_ENV)
            .map(|raw| Self::parse(&raw))
            .unwrap_or_default()
    }

    /// True if `ip` is in the operator-supplied extra blocklist (does not
    /// include the built-in ranges — combine with [`is_blocked_ip`]).
    pub fn blocks_ip(&self, ip: &IpAddr) -> bool {
        self.extra_ips.contains(ip)
    }

    /// True if `host` (the request's hostname, before resolution) is in the
    /// operator-supplied extra blocklist.
    pub fn blocks_host(&self, host: &str) -> bool {
        self.extra_hosts.contains(&host.to_lowercase())
    }
}

/// Returns true if `ip` must NOT be reachable from this plugin (loopback,
/// private/RFC1918, link-local, or the `169.254.169.254` cloud metadata
/// address). Called once per resolved IP for the request's host before any
/// network I/O happens.
///
pub fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_multicast()
                || v4.is_broadcast()
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unique_local()
                || v6.is_unicast_link_local()
                || v6.is_multicast()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn blocks_loopback() {
        assert!(is_blocked_ip(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))));
    }

    #[test]
    fn blocks_rfc1918_10() {
        assert!(is_blocked_ip(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
    }

    #[test]
    fn blocks_rfc1918_172() {
        assert!(is_blocked_ip(IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1))));
    }

    #[test]
    fn blocks_rfc1918_192() {
        assert!(is_blocked_ip(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))));
    }

    #[test]
    fn blocks_link_local() {
        assert!(is_blocked_ip(IpAddr::V4(Ipv4Addr::new(169, 254, 1, 1))));
    }

    #[test]
    fn blocks_cloud_metadata() {
        assert!(is_blocked_ip(IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254))));
    }

    #[test]
    fn allows_public_ip() {
        assert!(!is_blocked_ip(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
    }

    #[test]
    fn allows_another_public_ip() {
        assert!(!is_blocked_ip(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))));
    }

    #[test]
    fn blocklist_parses_ips_and_hosts() {
        let bl = Blocklist::parse("8.8.8.8, internal.corp , 1.1.1.1");
        assert!(bl.blocks_ip(&IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
        assert!(bl.blocks_ip(&IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))));
        assert!(bl.blocks_host("internal.corp"));
        assert!(bl.blocks_host("Internal.Corp"));
    }

    #[test]
    fn blocklist_ignores_blank_entries() {
        let bl = Blocklist::parse(" , ,");
        assert_eq!(bl, Blocklist::default());
    }

    #[test]
    fn blocklist_does_not_block_unlisted() {
        let bl = Blocklist::parse("8.8.8.8,internal.corp");
        assert!(!bl.blocks_ip(&IpAddr::V4(Ipv4Addr::new(9, 9, 9, 9))));
        assert!(!bl.blocks_host("example.com"));
    }

    #[test]
    fn blocklist_from_env_empty_when_unset() {
        std::env::remove_var(EXTRA_BLOCKLIST_ENV);
        assert_eq!(Blocklist::from_env(), Blocklist::default());
    }
}
