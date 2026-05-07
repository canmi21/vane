//! DNS resolver integration for upstream `Fetch`.
//!
//! Wraps `hickory_resolver::TokioResolver` behind a Tower
//! `Service<Name>` so it slots into
//! `hyper_util::client::legacy::connect::HttpConnector::new_with_resolver`.
//! Per-upstream nameserver override is expressed via [`DnsConfig`],
//! which participates in
//! [`crate::fetch::client_cache::ClientFingerprint`] so two fetches
//! with different nameserver lists land in distinct cache slots.
//!
//! See `spec/crates/engine.md` § _DNS resolver: hickory-resolver_
//! and `spec/crates/core.md` § _Rule schema_ (`dns` row).

use std::future::Future;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use hickory_resolver::TokioResolver;
use hickory_resolver::config::{NameServerConfig, ResolverConfig};
use hickory_resolver::net::runtime::TokioRuntimeProvider;
use hyper_util::client::legacy::connect::dns::Name;
use tower_service::Service;

/// Per-upstream DNS posture. Distinct variants get distinct
/// fingerprints so the daemon-level client cache never aliases two
/// fetches that would resolve through different nameserver sets.
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub enum DnsConfig {
	/// Read `/etc/resolv.conf` (and `/etc/hosts` via the
	/// `system-config` feature).
	System,
	/// Query only these nameservers, in the listed order. The order is
	/// load-bearing — `["1.1.1.1", "8.8.8.8"]` and the reverse get
	/// distinct cache slots so the operator's primary / secondary
	/// intent is honored. No fall-through to the system resolver on
	/// failure (see `spec/crates/core.md`).
	Custom(Vec<SocketAddr>),
}

/// Tower service over `hickory_resolver::TokioResolver`. The inner
/// `TokioResolver` caches by TTL, so per-dial resolver invocations
/// hit hickory's cache rather than the wire after the first lookup.
#[derive(Clone)]
pub struct HickoryDnsResolver {
	inner: Arc<TokioResolver>,
}

impl HickoryDnsResolver {
	/// Build the wrapper for `cfg`. Sync + cheap: hickory's
	/// `builder_*` paths read configuration into memory but do not
	/// open any sockets.
	///
	/// # Errors
	/// Any failure surfaces as `io::Error` (`Other` for build paths,
	/// matching `GaiResolver`'s error shape so the connector layer
	/// sees a uniform type).
	pub fn build(cfg: &DnsConfig) -> Result<Self, io::Error> {
		let resolver = match cfg {
			DnsConfig::System => TokioResolver::builder_tokio()
				.map_err(|e| io::Error::other(format!("hickory system config: {e}")))?
				.build()
				.map_err(|e| io::Error::other(format!("hickory build: {e}")))?,
			DnsConfig::Custom(nameservers) => {
				let mut resolver_cfg = ResolverConfig::default();
				for addr in nameservers {
					// `udp_and_tcp(ip)` builds NameServer with two ConnectionConfigs
					// (UDP primary, TCP fallback at the same port). hickory 0.26
					// keeps `port` on each ConnectionConfig — patch both so the
					// SocketAddr's port wins over hickory's default 53.
					let mut ns = NameServerConfig::udp_and_tcp(addr.ip());
					for conn in &mut ns.connections {
						conn.port = addr.port();
					}
					resolver_cfg.add_name_server(ns);
				}
				TokioResolver::builder_with_config(resolver_cfg, TokioRuntimeProvider::default())
					.build()
					.map_err(|e| io::Error::other(format!("hickory build: {e}")))?
			}
		};
		Ok(Self { inner: Arc::new(resolver) })
	}
}

impl HickoryDnsResolver {
	/// Resolve `host` to a single [`IpAddr`] via the inner hickory
	/// resolver. Used by code paths that already know their port and
	/// need a typed `IpAddr` (the H3 dial composes the result with the
	/// upstream's static port to feed `quinn::Endpoint::connect`).
	///
	/// IP literals short-circuit — `host.parse::<IpAddr>()` is tried
	/// first so configurations like `127.0.0.1:443` or `[::1]:443`
	/// don't issue a wire query (some hickory configurations would
	/// attempt PTR resolution otherwise).
	///
	/// # Errors
	///
	/// `io::Error::NotFound` when hickory returns successfully but the
	/// answer set is empty. Any underlying lookup failure is wrapped
	/// with `kind = NotFound` to match the `Service<Name>` shape.
	pub async fn resolve_first_ip(&self, host: &str) -> io::Result<IpAddr> {
		if let Ok(ip) = host.parse::<IpAddr>() {
			return Ok(ip);
		}
		let lookup = self
			.inner
			.lookup_ip(host)
			.await
			.map_err(|e| io::Error::new(io::ErrorKind::NotFound, format!("hickory: {e}")))?;
		lookup.iter().next().ok_or_else(|| {
			io::Error::new(io::ErrorKind::NotFound, format!("hickory: no addresses for {host:?}"))
		})
	}
}

impl Service<Name> for HickoryDnsResolver {
	type Response = std::vec::IntoIter<SocketAddr>;
	type Error = io::Error;
	type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

	fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
		Poll::Ready(Ok(()))
	}

	fn call(&mut self, name: Name) -> Self::Future {
		let resolver = Arc::clone(&self.inner);
		Box::pin(async move {
			let lookup = resolver
				.lookup_ip(name.as_str())
				.await
				.map_err(|e| io::Error::new(io::ErrorKind::NotFound, format!("hickory: {e}")))?;
			// Port 0 is intentional: hyper-util's HttpConnector replaces it
			// with the URI's port via `set_port` after this returns.
			let addrs: Vec<SocketAddr> = lookup.iter().map(|ip| SocketAddr::new(ip, 0)).collect();
			Ok(addrs.into_iter())
		})
	}
}

/// Parse `args.dns` into a [`DnsConfig`].
///
/// Accepts (per `spec/crates/core.md` § _Rule schema_):
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

	#[test]
	fn dns_config_eq_same_order() {
		let a = DnsConfig::Custom(vec!["1.1.1.1:53".parse().unwrap(), "8.8.8.8:53".parse().unwrap()]);
		let b = a.clone();
		assert_eq!(a, b);
	}

	#[test]
	fn dns_config_neq_different_order() {
		let a = DnsConfig::Custom(vec!["1.1.1.1:53".parse().unwrap(), "8.8.8.8:53".parse().unwrap()]);
		let b = DnsConfig::Custom(vec!["8.8.8.8:53".parse().unwrap(), "1.1.1.1:53".parse().unwrap()]);
		assert_ne!(a, b, "primary/secondary swap must produce a distinct fingerprint");
	}

	#[test]
	fn dns_config_neq_system_vs_custom() {
		let a = DnsConfig::System;
		let b = DnsConfig::Custom(vec!["1.1.1.1:53".parse().unwrap()]);
		assert_ne!(a, b);
	}

	#[test]
	fn build_system_resolver_succeeds() {
		HickoryDnsResolver::build(&DnsConfig::System).expect("system resolver builds");
	}

	#[test]
	fn build_custom_resolver_succeeds() {
		let cfg = DnsConfig::Custom(vec!["1.1.1.1:53".parse().unwrap()]);
		HickoryDnsResolver::build(&cfg).expect("custom resolver builds");
	}

	#[test]
	fn build_custom_resolver_with_ipv6_succeeds() {
		let cfg = DnsConfig::Custom(vec!["[2606:4700:4700::1111]:53".parse().unwrap()]);
		HickoryDnsResolver::build(&cfg).expect("ipv6 custom resolver builds");
	}

	#[tokio::test]
	async fn resolve_first_ip_short_circuits_ipv4_literal() {
		let r = HickoryDnsResolver::build(&DnsConfig::System).expect("build");
		let ip = r.resolve_first_ip("127.0.0.1").await.expect("ipv4 literal");
		assert_eq!(ip, IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
	}

	#[tokio::test]
	async fn resolve_first_ip_short_circuits_ipv6_literal() {
		let r = HickoryDnsResolver::build(&DnsConfig::System).expect("build");
		let ip = r.resolve_first_ip("::1").await.expect("ipv6 literal");
		assert_eq!(ip, IpAddr::V6(std::net::Ipv6Addr::LOCALHOST));
	}

	#[tokio::test]
	async fn resolve_first_ip_fails_on_unreachable_nameserver() {
		// 127.0.0.1:1 is the reserved tcpmux port; no DNS server runs
		// there. Forcing the resolver to point at it makes lookup fail
		// without depending on the host's network.
		let cfg = DnsConfig::Custom(vec!["127.0.0.1:1".parse().unwrap()]);
		let r = HickoryDnsResolver::build(&cfg).expect("build");
		let err = r.resolve_first_ip("nonexistent.invalid").await.expect_err("must fail");
		assert_eq!(err.kind(), io::ErrorKind::NotFound);
	}
}
