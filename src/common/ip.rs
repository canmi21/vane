/* src/common/ip.rs */

use std::net::IpAddr;

/// Checks if an IP address is within private, reserved, or loopback ranges.
pub fn is_private_ip(ip: &IpAddr) -> bool {
	match ip {
		IpAddr::V4(ipv4) => {
			ipv4.is_private()
				|| ipv4.is_loopback()
				|| ipv4.is_link_local()
				|| ipv4.is_unspecified()
				|| ipv4.is_documentation()
				|| ipv4.is_broadcast()
				|| (ipv4.octets()[0] == 100 && (ipv4.octets()[1] & 0b1100_0000) == 0b0100_0000) // Carrier-grade NAT
		}
		IpAddr::V6(ipv6) => {
			ipv6.is_loopback() || ipv6.is_unspecified() || ((ipv6.segments()[0] & 0xfe00) == 0xfc00) // Unique Local Addresses
		}
	}
}
