//! Args parsing, transport-specific dispatch construction, and the
//! public `factory` / `register` entry points. Built once per
//! `FetchInst` during the lower pass — everything in this file runs
//! at link time, not on the per-request hot path.

use std::sync::Arc;

use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::{TokioExecutor, TokioTimer};
use vane_core::{Body, FetchKind};

use super::{Dispatch, HttpProxyFetch, UpstreamVersion};
#[cfg(feature = "h3")]
use super::{H3_CONNECT_TIMEOUT_DEFAULT, QuicDispatchState};
use crate::factories::{FactoryError, FetchFactories};
use crate::fetch::client_cache::ClientFingerprint;
use crate::fetch::dns::{DnsConfig, HickoryDnsResolver, parse_dns_args};
use crate::fetch::pool;
use crate::fetch::retry::RetryPolicy;
use crate::fetch::upstream::{UpstreamTls, parse_tls_args};
use crate::flow_graph::FetchInst;

/// Split an `args.upstream` `host:port` string into its parts. The
/// returned host has surrounding brackets stripped (`[::1]` → `::1`)
/// so the resolver's IP-literal short-circuit reaches `IpAddr::parse`.
/// Returns the host owned (callers wrap it into `Arc<str>`); port is
/// validated as `u16`.
#[cfg(feature = "h3")]
fn split_host_port(upstream: &str) -> Result<(String, u16), String> {
	let (host_part, port_part) =
		upstream.rsplit_once(':').ok_or_else(|| "missing port".to_string())?;
	let host = host_part.strip_prefix('[').and_then(|s| s.strip_suffix(']')).unwrap_or(host_part);
	if host.is_empty() {
		return Err("empty host".to_string());
	}
	let port = port_part.parse::<u16>().map_err(|e| format!("invalid port: {e}"))?;
	Ok((host.to_owned(), port))
}

/// Build the per-instance pooled client. The connector accepts both
/// `http://` and `https://` URIs so a single `Client` handles
/// cleartext and TLS upstreams; the connector's `enable_http1` /
/// `enable_http2` toggles drive the ALPN list, and the legacy
/// builder's `http2_only` flag pins the post-handshake driver.
///
/// `hyper-rustls` rejects a pre-populated `alpn_protocols` on the
/// `ClientConfig` it receives (the connector builder reserves that
/// field for its own use), so the per-version ALPN restriction goes
/// through `enable_httpN` here, not through cloning the cached
/// `ClientConfig`.
fn build_client(
	version: UpstreamVersion,
	tls: Option<&UpstreamTls>,
	dns: &DnsConfig,
) -> Client<HttpsConnector<HttpConnector<HickoryDnsResolver>>, Body> {
	let tls_cfg = match tls {
		Some(t) => Arc::clone(&t.client_config),
		// Cleartext path never reaches the rustls handshake; supply a
		// minimal default config so `HttpsConnectorBuilder` is happy.
		// The connector picks the cleartext branch the moment it sees
		// an `http://` URI.
		None => Arc::new(
			rustls::ClientConfig::builder()
				.with_root_certificates(rustls::RootCertStore::empty())
				.with_no_client_auth(),
		),
	};

	// Resolver is built per Client; spec does not require global sharing
	// and (version, tls, dns) tuples are bounded in production.
	// Hickory's TTL cache lives inside this resolver instance.
	let resolver = HickoryDnsResolver::build(dns).expect("build hickory resolver");
	let mut http = HttpConnector::new_with_resolver(resolver);
	// Permit https:// URIs through the inner connector — TLS is wrapped
	// by hyper-rustls one layer up. Mirrors `HttpConnector::new`'s
	// posture for the GaiResolver path.
	http.enforce_http(false);
	// Bound TCP connect at the SLA documented in
	// `spec/crates/engine.md` § _Exhaustion defaults (per upstream)_.
	// Without this, hyper-util defaults to no connect deadline and a
	// slow DNS / route-blackhole upstream blocks fetches well past the
	// 5 s SLA. TLS handshake happens above this layer and has its own
	// budget threaded through `tokio-rustls`.
	http.set_connect_timeout(Some(pool::CONNECT_TIMEOUT));

	let connector_with_protocols =
		hyper_rustls::HttpsConnectorBuilder::new().with_tls_config((*tls_cfg).clone()).https_or_http();
	let https = match version {
		UpstreamVersion::Auto => {
			connector_with_protocols.enable_http1().enable_http2().wrap_connector(http)
		}
		UpstreamVersion::Http1 => connector_with_protocols.enable_http1().wrap_connector(http),
		UpstreamVersion::Http2 => connector_with_protocols.enable_http2().wrap_connector(http),
		// `Http3` is dispatched via the QuicPool, never through `build_client`.
		// The factory short-circuits before reaching here, so this arm is
		// unreachable in practice; keep it for exhaustiveness.
		#[cfg(feature = "h3")]
		UpstreamVersion::Http3 => {
			unreachable!("build_client is the TCP path; H3 dispatch goes through QuicPool")
		}
	};

	let mut builder = Client::builder(TokioExecutor::new());
	// Pool tunables from `spec/crates/engine.md` § _Exhaustion
	// defaults (per upstream)_. `pool_timer` is mandatory: without an
	// explicit timer source, hyper-util's idle-timeout machinery is a
	// no-op and connections accumulate until the OS or peer closes
	// them.
	builder
		.pool_max_idle_per_host(pool::MAX_IDLE_PER_HOST)
		.pool_idle_timeout(Some(pool::IDLE_TIMEOUT))
		.pool_timer(TokioTimer::new());
	match version {
		// Auto + Http1: hyper-util's legacy client defaults to H1.
		// On TLS the connector restricts ALPN to `http/1.1` for
		// `Http1`; on cleartext H1 is the default (no H2 upgrade
		// path on plain TCP).
		UpstreamVersion::Auto | UpstreamVersion::Http1 => {}
		UpstreamVersion::Http2 => {
			// Prior-knowledge h2c on cleartext, ALPN-h2 on TLS.
			builder.http2_only(true);
			// CVE-2023-44487 ("HTTP/2 Rapid Reset") mitigation: hyper
			// defaults to tracking up to 1024 pending-reset streams
			// per connection, which a misbehaving upstream can pin
			// memory for. Align with the idle-pool cap.
			builder.http2_max_concurrent_reset_streams(pool::H2_MAX_CONCURRENT_RESET_STREAMS);
		}
		#[cfg(feature = "h3")]
		UpstreamVersion::Http3 => {
			unreachable!("build_client is the TCP path; H3 dispatch goes through QuicPool")
		}
	}
	builder.build(https)
}

/// Fork point for `args.upstream_kind` (injected by the alias-
/// resolution layer in `vane_core::rule::TerminateSpec`, see
/// `spec/crates/engine.md` § _Concrete fetches_): socket-based aliases produce
/// `"tcp"`; the `cgi` alias produces `"cgi"`. Hand-rolled rules
/// without an alias fall through to the socket path for backwards
/// compatibility, but anything else is a hard error so
/// misconfiguration surfaces at link time rather than as a
/// misleading "missing upstream" downstream.
fn dispatch_upstream_kind(args: &serde_json::Value) -> Option<Result<FetchInst, FactoryError>> {
	match args.get("upstream_kind").and_then(serde_json::Value::as_str) {
		#[cfg(feature = "cgi")]
		Some("cgi") => Some(crate::fetch::cgi::factory(args)),
		#[cfg(not(feature = "cgi"))]
		Some("cgi") => Some(Err(FactoryError::Invalid(
			"upstream_kind 'cgi' requires the 'cgi' cargo feature, which is not active in this build"
				.to_string(),
		))),
		Some("tcp") | None => None,
		Some(other) => Some(Err(FactoryError::Invalid(format!(
			"args.upstream_kind must be 'tcp' or 'cgi' (or absent for backwards-compat with hand-written socket rules) — got {other:?}",
		)))),
	}
}

/// Args parser exposed as a registry-friendly factory.
///
/// Args shape:
///
/// ```json
/// {
///   "upstream": "host:port",
///   "version":  "auto" | "h1" | "h2" | "h3",
///   "tls": {
///     "verify_hostname":      "api.example.com",
///     "insecure_skip_verify": false
///   }
/// }
/// ```
///
/// `version` defaults to `"auto"`. `"h3"` is reserved for the future
/// `h3` cargo feature; factories on builds without it return an
/// error pointing operators at the right rebuild flag. `tls` is
/// optional — absent means cleartext upstream.
///
/// # Errors
/// Returns [`FactoryError`] when `upstream` is missing/empty, when
/// `version` is not one of the four accepted strings, when
/// `version: "h3"` is requested on a build without the `h3` feature,
/// or when the TLS client config fails to build.
pub fn factory(
	args: &serde_json::Value,
	crl_cache: Option<&Arc<crate::tls::CrlCache>>,
) -> Result<FetchInst, FactoryError> {
	if let Some(out) = dispatch_upstream_kind(args) {
		return out;
	}
	let upstream = args.get("upstream").and_then(serde_json::Value::as_str).ok_or_else(|| {
		FactoryError::Invalid("missing args.upstream (string \"host:port\")".to_string())
	})?;
	if upstream.is_empty() {
		return Err(FactoryError::Invalid("args.upstream must not be empty".to_string()));
	}
	let version = parse_version_arg(args)?;
	let tls = parse_tls_args(upstream, args.get("tls"), crl_cache)
		.map_err(|e| FactoryError::Invalid(format!("args.tls: {e}")))?;
	let dns =
		parse_dns_args(args.get("dns")).map_err(|e| FactoryError::Invalid(format!("args.dns: {e}")))?;

	if matches!(version, UpstreamVersion::Auto) && tls.is_none() {
		// Cleartext has no ALPN to negotiate on, so `auto` collapses
		// to H1. Surface the degradation so operators who actually
		// wanted h2c add `version: "h2"` explicitly.
		tracing::warn!(
			upstream,
			"cleartext upstream + version=auto: no ALPN to negotiate, falling back to h1; \
			 set version: h2 explicitly for prior-knowledge h2c",
		);
	}

	let retry = crate::fetch::retry::parse(args.get("retry"))
		.map_err(|e| FactoryError::Invalid(format!("args.retry: {e}")))?;

	#[cfg(feature = "h3")]
	if matches!(version, UpstreamVersion::Http3) {
		return build_h3_dispatch(args, upstream, version, tls, &dns, retry);
	}

	// TCP family — compute the cache key. The connector wires ALPN
	// via `enable_http1` / `enable_http2`, which is `version`-driven,
	// so we patch the version-specific ALPN list into the parsed TLS
	// fingerprint here (parse_tls_args has no `version` to consult).
	// Cleartext upstreams keep `tls: None` and still share by version.
	let alpn_protocols = match version {
		UpstreamVersion::Auto => vec![b"h2".to_vec(), b"http/1.1".to_vec()],
		UpstreamVersion::Http1 => vec![b"http/1.1".to_vec()],
		UpstreamVersion::Http2 => vec![b"h2".to_vec()],
		// Unreachable — the H3 branch above already returned.
		#[cfg(feature = "h3")]
		UpstreamVersion::Http3 => unreachable!("H3 dispatch returns above"),
	};
	let tls_fp = tls.as_ref().map(|t| {
		let mut fp = t.fingerprint.clone();
		fp.alpn_protocols = alpn_protocols;
		fp
	});
	let client_fp = ClientFingerprint { version, tls: tls_fp, dns: dns.clone() };
	let tls_for_build = tls.clone();
	let dns_for_build = dns.clone();
	let client = crate::fetch::client_cache::get_or_build(client_fp, move || {
		build_client(version, tls_for_build.as_ref(), &dns_for_build)
	});

	let scheme = if tls.is_some() { http::uri::Scheme::HTTPS } else { http::uri::Scheme::HTTP };
	let authority: http::uri::Authority = upstream.parse().map_err(|e| {
		FactoryError::Invalid(format!("args.upstream {upstream:?}: invalid authority: {e}"))
	})?;

	Ok(FetchInst::L7(Arc::new(HttpProxyFetch {
		version,
		scheme,
		authority,
		dispatch: Dispatch::Tcp(client),
		retry: Arc::new(retry),
	})))
}

/// Plug `FetchKind::HttpProxy` into a `FetchFactories` registry. The
/// `crl_cache` is captured by the registered closure so each factory
/// invocation routes through the daemon-wide cache.
pub fn register(factories: &mut FetchFactories, crl_cache: Option<Arc<crate::tls::CrlCache>>) {
	factories.register(FetchKind::HttpProxy, move |args| factory(args, crl_cache.as_ref()));
}

/// Parse `args.version` (default `"auto"`) into [`UpstreamVersion`].
/// `"h3"` on a build without the `h3` feature surfaces the rebuild
/// hint at factory time so operators don't get a less specific link
/// error downstream.
fn parse_version_arg(args: &serde_json::Value) -> Result<UpstreamVersion, FactoryError> {
	match args.get("version").and_then(serde_json::Value::as_str).unwrap_or("auto") {
		"auto" => Ok(UpstreamVersion::Auto),
		"h1" => Ok(UpstreamVersion::Http1),
		"h2" => Ok(UpstreamVersion::Http2),
		#[cfg(feature = "h3")]
		"h3" => Ok(UpstreamVersion::Http3),
		#[cfg(not(feature = "h3"))]
		"h3" => Err(FactoryError::Invalid(
			"version 'h3' requires the 'h3' cargo feature, which is not active in this build".to_string(),
		)),
		other => Err(FactoryError::Invalid(format!(
			"args.version must be one of 'auto' / 'h1' / 'h2' / 'h3' — got {other:?}"
		))),
	}
}

/// Build the H3 dispatch state and wrap it as a [`FetchInst::L7`].
/// TLS is mandatory (RFC 9114 mandates QUIC + TLS 1.3); cleartext H3
/// is rejected at factory time. The rustls config is cloned and ALPN
/// is pinned to `[b"h3"]` since the QUIC pool embeds ALPN into the
/// rustls config (vs the hyper-rustls connector's `enable_httpN`).
#[cfg(feature = "h3")]
fn build_h3_dispatch(
	args: &serde_json::Value,
	upstream: &str,
	version: UpstreamVersion,
	tls: Option<UpstreamTls>,
	dns: &DnsConfig,
	retry: RetryPolicy,
) -> Result<FetchInst, FactoryError> {
	let tls = tls.ok_or_else(|| {
		FactoryError::Invalid("version 'h3' requires args.tls (h3 mandates QUIC + TLS 1.3)".to_string())
	})?;
	let mut h3_rustls: rustls::ClientConfig = (*tls.client_config).clone();
	h3_rustls.alpn_protocols = vec![b"h3".to_vec()];
	let h3_rustls = Arc::new(h3_rustls);
	let mut tls_fp = tls.fingerprint.clone();
	tls_fp.alpn_protocols = vec![b"h3".to_vec()];
	let connect_timeout = match args.get("connect_timeout").and_then(serde_json::Value::as_str) {
		Some(s) => crate::fetch::retry::parse_duration(s)
			.map_err(|e| FactoryError::Invalid(format!("args.connect_timeout: {e}")))?,
		None => H3_CONNECT_TIMEOUT_DEFAULT,
	};
	let resolver = HickoryDnsResolver::build(dns)
		.map_err(|e| FactoryError::Invalid(format!("args.dns hickory build: {e}")))?;
	let (host, port) = split_host_port(upstream)
		.map_err(|e| FactoryError::Invalid(format!("args.upstream {upstream:?}: {e}")))?;
	let dispatch = Dispatch::Quic(QuicDispatchState {
		rustls_cfg: h3_rustls,
		sni: Arc::from(tls.verify_hostname.as_str()),
		tls_fp,
		connect_timeout,
		resolver: Arc::new(resolver),
		host: Arc::from(host.as_str()),
		port,
	});
	let authority: http::uri::Authority = upstream.parse().map_err(|e| {
		FactoryError::Invalid(format!("args.upstream {upstream:?}: invalid authority: {e}"))
	})?;
	Ok(FetchInst::L7(Arc::new(HttpProxyFetch {
		version,
		scheme: http::uri::Scheme::HTTPS,
		authority,
		dispatch,
		retry: Arc::new(retry),
	})))
}

#[cfg(test)]
mod tests {
	use super::*;

	fn install_crypto() {
		crate::crypto::install_default_provider();
	}

	#[test]
	fn factory_rejects_missing_upstream() {
		install_crypto();
		match factory(&serde_json::json!({}), None) {
			Ok(_) => panic!("must reject missing upstream"),
			Err(e) => assert!(e.message().contains("upstream"), "{}", e.message()),
		}
	}

	#[test]
	fn factory_rejects_empty_upstream() {
		install_crypto();
		match factory(&serde_json::json!({ "upstream": "" }), None) {
			Ok(_) => panic!("must reject empty upstream"),
			Err(e) => assert!(e.message().contains("must not be empty"), "{}", e.message()),
		}
	}

	#[test]
	fn factory_rejects_tls_with_insecure_skip_verify_when_env_unset() {
		// Per the spec's master-switch contract: `insecure_skip_verify`
		// in config alone is insufficient — VANE_ALLOW_INSECURE_UPSTREAM=1
		// has to be set in the daemon env. The unit-test environment
		// never sets that, so the factory must refuse the config.
		install_crypto();
		let Err(FactoryError::Invalid(msg)) = factory(
			&serde_json::json!({
				"upstream": "127.0.0.1:9443",
				"tls": { "insecure_skip_verify": true, "verify_hostname": "localhost" },
			}),
			None,
		) else {
			panic!("factory must reject insecure tls config without env opt-in");
		};
		assert!(msg.contains("VANE_ALLOW_INSECURE_UPSTREAM"), "error names env var: {msg}");
	}

	#[cfg(not(feature = "h3"))]
	#[test]
	fn factory_rejects_version_h3_without_feature() {
		install_crypto();
		let Err(FactoryError::Invalid(msg)) = factory(
			&serde_json::json!({
				"upstream": "127.0.0.1:9443",
				"version": "h3",
			}),
			None,
		) else {
			panic!("h3 must be rejected on builds without the feature");
		};
		assert!(msg.contains("h3"), "error names the missing feature: {msg}");
	}

	#[cfg(feature = "h3")]
	#[test]
	fn factory_rejects_h3_without_tls() {
		install_crypto();
		// H3 mandates QUIC + TLS 1.3 (RFC 9114) — the factory rejects
		// `version: "h3"` without `args.tls` even with the cargo
		// feature enabled.
		let Err(FactoryError::Invalid(msg)) = factory(
			&serde_json::json!({
				"upstream": "127.0.0.1:9443",
				"version": "h3",
			}),
			None,
		) else {
			panic!("h3 without tls must be rejected");
		};
		assert!(msg.contains("h3") && msg.contains("tls"), "error names h3 + tls: {msg}");
	}

	#[cfg(feature = "h3")]
	#[test]
	fn factory_rejects_h3_with_insecure_skip_verify_when_env_unset() {
		// Same master-switch contract as the H1/H2 path: H3 + TLS with
		// `insecure_skip_verify` is rejected without the env opt-in.
		install_crypto();
		let Err(FactoryError::Invalid(msg)) = factory(
			&serde_json::json!({
				"upstream": "127.0.0.1:9443",
				"version": "h3",
				"tls": { "insecure_skip_verify": true, "verify_hostname": "localhost" },
			}),
			None,
		) else {
			panic!("h3 + insecure must be rejected without env opt-in");
		};
		assert!(msg.contains("VANE_ALLOW_INSECURE_UPSTREAM"), "error names env var: {msg}");
	}

	#[test]
	fn factory_rejects_unknown_version() {
		install_crypto();
		let Err(FactoryError::Invalid(msg)) = factory(
			&serde_json::json!({
				"upstream": "127.0.0.1:9443",
				"version": "h7",
			}),
			None,
		) else {
			panic!("unknown version must be rejected");
		};
		assert!(msg.contains("auto") && msg.contains("h1"), "{msg}");
	}

	#[test]
	fn factory_accepts_explicit_h1_version() {
		install_crypto();
		let result = factory(
			&serde_json::json!({
				"upstream": "127.0.0.1:9443",
				"version": "h1",
			}),
			None,
		);
		assert!(result.is_ok(), "h1 version must build");
	}

	#[test]
	fn factory_accepts_explicit_h2_cleartext() {
		install_crypto();
		let result = factory(
			&serde_json::json!({
				"upstream": "127.0.0.1:9443",
				"version": "h2",
			}),
			None,
		);
		assert!(result.is_ok(), "h2 cleartext (h2c) must build");
	}

	#[cfg(feature = "h3")]
	#[test]
	fn split_host_port_accepts_ipv4() {
		assert_eq!(split_host_port("127.0.0.1:443").unwrap(), ("127.0.0.1".to_owned(), 443));
	}

	#[cfg(feature = "h3")]
	#[test]
	fn split_host_port_strips_ipv6_brackets() {
		assert_eq!(split_host_port("[::1]:8443").unwrap(), ("::1".to_owned(), 8443));
	}

	#[cfg(feature = "h3")]
	#[test]
	fn split_host_port_accepts_dns_name() {
		assert_eq!(
			split_host_port("api.example.com:443").unwrap(),
			("api.example.com".to_owned(), 443),
		);
	}

	#[cfg(feature = "h3")]
	#[test]
	fn split_host_port_rejects_no_port() {
		assert!(split_host_port("127.0.0.1").is_err());
	}

	#[cfg(feature = "h3")]
	#[test]
	fn split_host_port_rejects_bad_port() {
		assert!(split_host_port("127.0.0.1:abc").is_err());
	}

	#[cfg(feature = "h3")]
	#[test]
	fn split_host_port_rejects_empty_host() {
		assert!(split_host_port(":443").is_err());
	}
}
