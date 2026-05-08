//! A `tower::Service<hyper_util::client::legacy::connect::dns::Name>`
//! adapter around `hickory_resolver::TokioResolver`, so a hickory
//! resolver can plug into `hyper_util::client::legacy::connect::HttpConnector::new_with_resolver`
//! and replace hyper's default blocking `GaiResolver`.
//!
//! See the README for the gap this fills (no public hickory ↔
//! hyper-util bridge).

#![deny(unsafe_code)]
#![warn(unreachable_pub)]

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

/// DNS posture for [`HickoryDnsResolver::build`]. Variants are
/// distinguished structurally — order of the nameserver list is
/// load-bearing for callers that key client caches by config (the
/// derived `Hash` / `PartialEq` honour list ordering).
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub enum DnsConfig {
	/// Read `/etc/resolv.conf` (and `/etc/hosts` via hickory's
	/// `system-config` feature, which this crate enables).
	System,
	/// Query only these nameservers, in the listed order. The order is
	/// load-bearing — `["1.1.1.1:53", "8.8.8.8:53"]` and the reverse
	/// produce distinct fingerprints. No fall-through to the system
	/// resolver on failure.
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

	/// Resolve `host` to a single [`IpAddr`] via the inner hickory
	/// resolver. Used by code paths that already know their port and
	/// need a typed `IpAddr` (e.g. an H3 dial that composes the result
	/// with a static port to feed `quinn::Endpoint::connect`).
	///
	/// IP literals short-circuit — `host.parse::<IpAddr>()` is tried
	/// first so configurations like `127.0.0.1` or `::1` don't issue a
	/// wire query (some hickory configurations would attempt PTR
	/// resolution otherwise).
	///
	/// # Errors
	/// `io::Error::NotFound` when hickory returns successfully but the
	/// answer set is empty. Any underlying lookup failure is wrapped
	/// with `kind = NotFound` to match the [`Service<Name>`] shape.
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

#[cfg(test)]
mod tests {
	use super::*;

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
