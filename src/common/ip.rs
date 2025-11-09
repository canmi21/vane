/* src/common/ip.rs */

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Checks if an IPv4 address is within any private, reserved, or special-use range.
fn is_private_ipv4(ipv4: &Ipv4Addr) -> bool {
	ipv4.is_private()
        || ipv4.is_loopback()
        || ipv4.is_link_local()
        || ipv4.is_unspecified()
        || ipv4.is_documentation()
        || ipv4.is_broadcast()
        // Carrier-grade NAT (100.64.0.0/10)
        || (ipv4.octets()[0] == 100 && (ipv4.octets()[1] & 0b1100_0000) == 0b0100_0000)
}

/// Checks if an IPv6 address is not a globally routable public address using stable methods.
///
/// This manual implementation is used because the idiomatic `!ipv6.is_global()`
/// method is currently an unstable feature in the Rust standard library.
/// See: https://github.com/rust-lang/rust/issues/27709
fn is_private_ipv6(ipv6: &Ipv6Addr) -> bool {
	ipv6.is_unspecified()
        || ipv6.is_loopback()
        // Unique-local (fc00::/7)
        || (ipv6.segments()[0] & 0xfe00) == 0xfc00
        // Link-local (fe80::/10)
        || (ipv6.segments()[0] & 0xffc0) == 0xfe80
        // Documentation (2001:db8::/32)
        || (ipv6.segments()[0] == 0x2001 && ipv6.segments()[1] == 0x0db8)
}

/// Checks if an IP address is considered private or non-routable on the public internet.
///
/// This function serves as the main entry point, delegating the check to the
/// appropriate helper based on the IP version (v4 or v6).
pub fn is_private_ip(ip: &IpAddr) -> bool {
	match ip {
		IpAddr::V4(ipv4) => is_private_ipv4(ipv4),
		IpAddr::V6(ipv6) => is_private_ipv6(ipv6),
	}
}
