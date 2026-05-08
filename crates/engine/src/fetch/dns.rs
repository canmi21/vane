//! DNS resolver integration for upstream `Fetch`.
//!
//! Re-exports [`hickory_tower_resolver::DnsConfig`] and
//! [`hickory_tower_resolver::HickoryDnsResolver`] (the project-agnostic
//! tower bridge) and pairs them with vane's JSON schema parser
//! [`parse_dns_args`]. `DnsConfig` participates in
//! [`crate::fetch::client_cache::ClientFingerprint`] so two fetches
//! with different nameserver lists land in distinct cache slots.
//!
//! See `spec/crates/engine.md` § _DNS_
//! and `spec/crates/core.md` § _Compile pipeline_ (`dns` row).

use std::net::{IpAddr, SocketAddr};

pub use hickory_tower_resolver::{DnsConfig, HickoryDnsResolver};

/// Parse `args.dns` into a [`DnsConfig`].
///
/// Accepts (per `spec/crates/core.md` § _Compile pipeline_):
/// - missing / `null` / `"system"` / `{}` → [`DnsConfig::System`]
/// - `{ "nameservers": [] }` → [`DnsConfig::System`] (semantic equiv of `{}`)
/// - `{ "nameservers": [...] }` non-empty → [`DnsConfig::Custom`]
///
/// # Errors
/// String description of any schema violation. Returned as `String`
/// because this runs at fetch-factory link time, where lighter-weight
/// errors are preferred over the full `vane_core::Error` shape.
pub fn parse_dns_args(args: Option<&serde_json::Value>) -> Result<DnsConfig, String> {
	let Some(args) = args else { return Ok(DnsConfig::System) };
	if args.is_null() {
		return Ok(DnsConfig::System);
	}
	if let Some(s) = args.as_str() {
		if s == "system" {
			return Ok(DnsConfig::System);
		}
		return Err(format!("dns string must be 'system', got {s:?}"));
	}
	let obj = args.as_object().ok_or("dns must be 'system' or an object")?;
	let Some(ns) = obj.get("nameservers") else {
		return Ok(DnsConfig::System);
	};
	let arr = ns.as_array().ok_or("dns.nameservers must be an array of strings")?;
	if arr.is_empty() {
		return Ok(DnsConfig::System);
	}
	let mut socks = Vec::with_capacity(arr.len());
	for entry in arr {
		let s = entry.as_str().ok_or("dns.nameservers entries must be strings")?;
		socks.push(parse_nameserver(s)?);
	}
	Ok(DnsConfig::Custom(socks))
}

fn parse_nameserver(s: &str) -> Result<SocketAddr, String> {
	if let Ok(addr) = s.parse::<SocketAddr>() {
		return Ok(addr);
	}
	// IP-only fallback, IPv4 only. Bare IPv6 like `::1` is ambiguous
	// between "host" and "host:port" shorthand, so we require operators
	// to write `[::1]:53` explicitly.
	if s.contains(':') {
		return Err(format!(
			"invalid nameserver {s:?}: bare IPv6 is rejected, write [IPv6]:port (e.g. [::1]:53)"
		));
	}
	s.parse::<IpAddr>()
		.map(|ip| SocketAddr::new(ip, 53))
		.map_err(|e| format!("invalid nameserver {s:?}: {e}"))
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	#[test]
	fn parse_missing_yields_system() {
		assert!(matches!(parse_dns_args(None).expect("none"), DnsConfig::System));
	}

	#[test]
	fn parse_null_yields_system() {
		assert!(matches!(
			parse_dns_args(Some(&serde_json::Value::Null)).expect("null"),
			DnsConfig::System
		));
	}

	#[test]
	fn parse_string_system_yields_system() {
		assert!(matches!(parse_dns_args(Some(&json!("system"))).expect("ok"), DnsConfig::System));
	}

	#[test]
	fn parse_empty_object_yields_system() {
		assert!(matches!(parse_dns_args(Some(&json!({}))).expect("ok"), DnsConfig::System));
	}

	#[test]
	fn parse_dns_string_other_than_system_rejected() {
		let err = parse_dns_args(Some(&json!("google"))).expect_err("must reject");
		assert!(err.contains("'system'"), "{err}");
	}

	#[test]
	fn parse_object_without_nameservers_yields_system() {
		let parsed = parse_dns_args(Some(&json!({ "irrelevant": true }))).expect("ok");
		assert!(matches!(parsed, DnsConfig::System));
	}

	#[test]
	fn parse_empty_nameservers_array_yields_system() {
		let parsed = parse_dns_args(Some(&json!({ "nameservers": [] }))).expect("ok");
		assert!(matches!(parsed, DnsConfig::System));
	}

	#[test]
	fn parse_nameservers_list_yields_custom_in_order() {
		let parsed =
			parse_dns_args(Some(&json!({ "nameservers": ["1.1.1.1", "8.8.8.8"] }))).expect("ok");
		match parsed {
			DnsConfig::Custom(v) => {
				assert_eq!(v.len(), 2);
				assert_eq!(v[0].to_string(), "1.1.1.1:53");
				assert_eq!(v[1].to_string(), "8.8.8.8:53");
			}
			DnsConfig::System => panic!("expected Custom"),
		}
	}

	#[test]
	fn parse_ipv4_with_port() {
		let parsed = parse_dns_args(Some(&json!({ "nameservers": ["1.1.1.1:5353"] }))).expect("ok");
		match parsed {
			DnsConfig::Custom(v) => assert_eq!(v[0].to_string(), "1.1.1.1:5353"),
			DnsConfig::System => panic!("expected Custom"),
		}
	}

	#[test]
	fn parse_ipv6_explicit_brackets_with_port() {
		let parsed = parse_dns_args(Some(&json!({ "nameservers": ["[::1]:53"] }))).expect("ok");
		match parsed {
			DnsConfig::Custom(v) => assert_eq!(v[0].to_string(), "[::1]:53"),
			DnsConfig::System => panic!("expected Custom"),
		}
	}

	#[test]
	fn parse_bare_ipv6_rejected() {
		let err = parse_dns_args(Some(&json!({ "nameservers": ["::1"] }))).expect_err("rejected");
		assert!(err.contains("[IPv6]:port"), "{err}");
	}

	#[test]
	fn parse_garbage_string_rejected() {
		let err = parse_dns_args(Some(&json!({ "nameservers": ["not-an-ip"] }))).expect_err("rejected");
		assert!(err.contains("invalid nameserver"), "{err}");
	}

	#[test]
	fn parse_nameservers_must_be_array() {
		let err = parse_dns_args(Some(&json!({ "nameservers": "1.1.1.1" }))).expect_err("rejected");
		assert!(err.contains("array"), "{err}");
	}

	#[test]
	fn parse_nameservers_entries_must_be_strings() {
		let err = parse_dns_args(Some(&json!({ "nameservers": [42] }))).expect_err("rejected");
		assert!(err.contains("strings"), "{err}");
	}

	#[test]
	fn parse_root_must_be_object_or_string() {
		let err = parse_dns_args(Some(&json!(42))).expect_err("rejected");
		assert!(err.contains("object"), "{err}");
	}
}
