/* src/common/net/ip.rs */

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Check if an IPv4 address is in a private range.
fn is_private_ipv4(ipv4: Ipv4Addr) -> bool {
	ipv4.is_private()
		|| ipv4.is_loopback()
		|| ipv4.is_link_local()
		|| ipv4.is_unspecified()
		|| ipv4.is_broadcast()
		|| ipv4.is_documentation()
		|| (ipv4.octets()[0] == 100 && (ipv4.octets()[1] & 0xc0) == 64)
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
#[must_use]
pub fn is_private_ip(ip: &IpAddr) -> bool {
	match ip {
		IpAddr::V4(ipv4) => is_private_ipv4(*ipv4),
		IpAddr::V6(ipv6) => is_private_ipv6(ipv6),
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::net::{Ipv4Addr, Ipv6Addr};

	/// Tests various private and special-use IPv4 address ranges.
	#[test]
	fn test_ipv4_private_ranges() {
		// Private networks
		assert!(is_private_ipv4(Ipv4Addr::new(10, 0, 0, 1)));
		assert!(is_private_ipv4(Ipv4Addr::new(172, 16, 0, 1)));
		assert!(is_private_ipv4(Ipv4Addr::new(192, 168, 1, 1)));

		// Special-use addresses
		assert!(is_private_ipv4(Ipv4Addr::new(127, 0, 0, 1))); // Loopback
		assert!(is_private_ipv4(Ipv4Addr::new(169, 254, 0, 1))); // Link-local
		assert!(is_private_ipv4(Ipv4Addr::new(0, 0, 0, 0))); // Unspecified
		assert!(is_private_ipv4(Ipv4Addr::new(192, 0, 2, 1))); // Documentation
		assert!(is_private_ipv4(Ipv4Addr::new(255, 255, 255, 255))); // Broadcast
		assert!(is_private_ipv4(Ipv4Addr::new(100, 64, 0, 1))); // Carrier-grade NAT

		// Public address
		assert!(!is_private_ipv4(Ipv4Addr::new(8, 8, 8, 8)));
	}

	/// Tests various non-global and special-use IPv6 address ranges.
	#[test]
	fn test_ipv6_private_ranges() {
		// Special-use addresses
		assert!(is_private_ipv6(&Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 0))); // Unspecified (::)
		assert!(is_private_ipv6(&Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1))); // Loopback (::1)
		assert!(is_private_ipv6(&Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 1))); // Unique-local
		assert!(is_private_ipv6(&Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1))); // Link-local
		assert!(is_private_ipv6(&Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, 1))); // Documentation

		// Public address
		assert!(!is_private_ipv6(&Ipv6Addr::new(0x2001, 0x4860, 0x4860, 0, 0, 0, 0, 0x8888)));
	}

	/// Tests the main dispatcher function `is_private_ip` for both v4 and v6.
	#[test]
	fn test_is_private_ip_delegation() {
		// IPv4 checks
		let private_ipv4 = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100));
		let public_ipv4 = IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1));
		assert!(is_private_ip(&private_ipv4));
		assert!(!is_private_ip(&public_ipv4));

		// IPv6 checks
		let private_ipv6 = IpAddr::V6(Ipv6Addr::new(0xfd12, 0x3456, 0x789a, 0, 0, 0, 0, 1));
		let public_ipv6 = IpAddr::V6(Ipv6Addr::new(0x2606, 0x4700, 0x4700, 0, 0, 0, 0, 1111));
		assert!(is_private_ip(&private_ipv6));
		assert!(!is_private_ip(&public_ipv6));
	}
}
