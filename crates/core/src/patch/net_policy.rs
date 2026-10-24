//! Network policy shared by patch downloaders. HTTP clients must pin validated IPs.
use std::net::{IpAddr, SocketAddr};

pub const PRIVATE_URL: &str = "patch URL resolves to a private network address; download refused";
pub const PRIVATE_REDIRECT: &str =
    "patch URL redirects to a private network address; download refused";
pub const DOWNGRADE: &str = "patch URL redirects from https to http; download refused";
pub const REDIRECT_LIMIT: &str = "patch URL exceeded the redirect limit; download refused";
pub const INVALID_REDIRECT: &str = "patch URL has an invalid redirect target; download refused";
pub const DNS_FAILED: &str = "patch URL host could not be resolved; download refused";
pub const MAX_REDIRECTS: usize = 5;

/// Check before resolving or connecting, including the first URL.
pub fn validate_scheme(scheme: &str, previous: Option<&str>) -> Result<(), &'static str> {
    if !matches!(scheme, "http" | "https") {
        return Err("only http/https zip_url allowed");
    }
    if previous == Some("https") && scheme == "http" {
        return Err(DOWNGRADE);
    }
    Ok(())
}

/// Reject local destinations, including IPv4 embedded in IPv6 literals.
pub fn is_public_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(ip) => {
            let octets = ip.octets();
            !ip.is_loopback()
                && !ip.is_link_local()
                && !ip.is_private()
                && !ip.is_unspecified()
                && !ip.is_multicast()
                && !ip.is_broadcast()
                && octets[0] != 0
                && octets[0] < 240
                // Shared carrier space can also route to internal services.
                && !(octets[0] == 100 && (64..=127).contains(&octets[1]))
        }
        IpAddr::V6(ip) => {
            if let Some(ipv4) = ip.to_ipv4() {
                return is_public_address(IpAddr::V4(ipv4));
            }
            !ip.is_loopback()
                && !ip.is_unspecified()
                && !ip.is_multicast()
                && (ip.segments()[0] & 0xffc0) != 0xfe80
                && (ip.segments()[0] & 0xfe00) != 0xfc00
                && (ip.segments()[0] & 0xffc0) != 0xfec0
        }
    }
}

/// Fail closed if DNS returns no addresses or even one disallowed answer.
/// `allow` is injected by local mock tests; production uses `is_public_address`.
pub fn validate_addresses<F: Fn(SocketAddr) -> bool>(
    addresses: &[SocketAddr],
    redirect: bool,
    allow: F,
) -> Result<(), &'static str> {
    if addresses.is_empty() {
        return Err(DNS_FAILED);
    }
    if addresses.iter().any(|address| !allow(*address)) {
        return Err(if redirect {
            PRIVATE_REDIRECT
        } else {
            PRIVATE_URL
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_network_policy_refuses_non_public_ipv4_and_ipv6() {
        for address in [
            "127.0.0.1",
            "127.255.255.255",
            "169.254.169.254",
            "169.254.255.255",
            "10.0.0.1",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "0.0.0.0",
            "224.0.0.1",
            "239.255.255.255",
            "255.255.255.255",
            "100.64.0.1",
            "::",
            "::1",
            "fe80::1",
            "febf::1",
            "fc00::1",
            "fdff::1",
            "ff02::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "::ffff:169.254.169.254",
            "::127.0.0.1",
        ] {
            assert!(!is_public_address(address.parse().unwrap()), "{address}");
        }
    }

    #[test]
    fn patch_network_policy_allows_public_ipv4_and_ipv6() {
        for address in [
            "8.8.8.8",
            "1.1.1.1",
            "172.15.255.255",
            "172.32.0.1",
            "2001:4860:4860::8888",
            "2606:4700:4700::1111",
            "::ffff:8.8.8.8",
        ] {
            assert!(is_public_address(address.parse().unwrap()), "{address}");
        }
    }

    #[test]
    fn patch_network_policy_checks_every_dns_answer() {
        let addresses = [
            "8.8.8.8:80".parse().unwrap(),
            "127.0.0.1:80".parse().unwrap(),
        ];
        assert_eq!(
            validate_addresses(&addresses, false, |a| is_public_address(a.ip())),
            Err(PRIVATE_URL)
        );
        assert_eq!(
            validate_addresses(&addresses, true, |a| is_public_address(a.ip())),
            Err(PRIVATE_REDIRECT)
        );
        assert_eq!(validate_addresses(&[], false, |_| true), Err(DNS_FAILED));
    }

    #[test]
    fn patch_network_policy_refuses_https_downgrade_and_non_http_hops() {
        assert_eq!(validate_scheme("http", Some("https")), Err(DOWNGRADE));
        for scheme in ["file", "ftp", "data"] {
            assert!(validate_scheme(scheme, None).is_err());
            assert!(validate_scheme(scheme, Some("http")).is_err());
        }
        for (scheme, previous) in [
            ("https", Some("http")),
            ("https", Some("https")),
            ("http", Some("http")),
            ("http", None),
            ("https", None),
        ] {
            assert_eq!(validate_scheme(scheme, previous), Ok(()));
        }
    }
}
