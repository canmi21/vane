//! Engine-side glue for the daemon-wide CRL cache. The cache itself
//! and the refreshable rustls verifiers live in the standalone
//! [`rustls_crl_refresh`] crate; this module supplies the
//! engine-specific pieces:
//!
//! * [`DefaultCrlFetcher`] — fetches via the engine's own hyper /
//!   hickory client stack and the daemon's system trust store.
//! * [`collect_upstream_crl_sources`] / [`collect_listener_crl_sources`]
//!   — walk a `vane_core::SymbolicFlowGraph` / a `BTreeMap` of
//!   listener TLS specs to gather every CRL source the `FlowGraph`
//!   names.
//!
//! See `spec/crates/engine-tls.md` § _CRL_ for fetch cadence,
//! failure handling, and the identity-not-content fingerprint.

use std::sync::Arc;

use async_trait::async_trait;
use http_body_util::BodyExt as _;
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
pub use rustls_crl_refresh::{
	CrlCache, CrlError, CrlFetchFailure, CrlFetcher, CrlSourceId, dedupe_crl_sources,
};
use vane_core::Body;

use crate::fetch::dns::{DnsConfig, HickoryDnsResolver};

const URL_BODY_LIMIT: usize = 16 * 1024 * 1024;

/// Production [`CrlFetcher`]: file via `tokio::fs`, URL via a
/// hyper-util `legacy::Client` over `hyper-rustls` with the system
/// trust store. Trust posture is the daemon default — there is no
/// per-source `insecure_skip_verify`.
pub struct DefaultCrlFetcher {
	client: Client<HttpsConnector<HttpConnector<HickoryDnsResolver>>, Body>,
}

impl DefaultCrlFetcher {
	/// Build the shared HTTP client. Mirrors
	/// [`crate::wasm_fetch::HyperHttpFetchBackend`]'s verified-path
	/// construction.
	///
	/// # Errors
	///
	/// String description when the system trust store or DNS resolver
	/// fails to construct.
	pub fn new() -> Result<Self, String> {
		let tls_cfg = crate::fetch::upstream::build_client_config(false)?;
		let resolver = HickoryDnsResolver::build(&DnsConfig::System)
			.map_err(|e| format!("hickory resolver: {e}"))?;
		let mut http = HttpConnector::new_with_resolver(resolver);
		http.enforce_http(false);
		let https = hyper_rustls::HttpsConnectorBuilder::new()
			.with_tls_config((*tls_cfg).clone())
			.https_or_http()
			.enable_http1()
			.enable_http2()
			.wrap_connector(http);
		let client = Client::builder(TokioExecutor::new()).build(https);
		Ok(Self { client })
	}

	/// `Arc`-shared variant. Daemons that inject an
	/// `Arc<dyn CrlFetcher>` use this directly.
	///
	/// # Errors
	/// As [`Self::new`].
	pub fn new_arc() -> Result<Arc<Self>, String> {
		Ok(Arc::new(Self::new()?))
	}
}

#[async_trait]
impl CrlFetcher for DefaultCrlFetcher {
	async fn fetch(&self, src: &CrlSourceId) -> Result<Vec<u8>, CrlError> {
		match src {
			CrlSourceId::File(path) => rustls_crl_refresh::read_crl_file(path).await,
			CrlSourceId::Url(url) => self.fetch_url(src, url).await,
		}
	}
}

impl DefaultCrlFetcher {
	async fn fetch_url(&self, src: &CrlSourceId, url: &str) -> Result<Vec<u8>, CrlError> {
		let uri: hyper::Uri =
			url.parse().map_err(|e| CrlError::fetch(src, format!("parse url: {e}")))?;
		let req = hyper::Request::get(uri)
			.header(hyper::header::ACCEPT, "application/pkix-crl, application/x-pkcs7-crl, */*")
			.body(Body::Empty)
			.map_err(|e| CrlError::fetch(src, format!("build request: {e}")))?;
		let resp = self
			.client
			.request(req)
			.await
			.map_err(|e| CrlError::fetch(src, format!("http request: {e}")))?;
		if !resp.status().is_success() {
			return Err(CrlError::fetch(src, format!("http {} for {url}", resp.status())));
		}
		let collected = http_body_util::Limited::new(resp.into_body(), URL_BODY_LIMIT)
			.collect()
			.await
			.map_err(|e| CrlError::fetch(src, format!("body read: {e}")))?;
		Ok(collected.to_bytes().to_vec())
	}
}

/// Walk a fully-symbolic flow graph and gather every CRL source named
/// by an HTTP-proxy or WebSocket-upgrade fetch's `args.tls.crls`.
/// Listener-side sources are collected separately by
/// [`collect_listener_crl_sources`] because they live in the parsed
/// [`vane_core::rule::ListenerTlsSpec`], not in raw fetch args.
///
/// Errors in the source schema are skipped silently here — invalid
/// shapes are caught at link time when `parse_tls_args` runs against
/// the same value. The link step is the authoritative parser; this
/// pass is just a best-effort pre-link source enumeration so the
/// daemon can register everything with the cache before the first
/// handshake.
#[must_use]
pub fn collect_upstream_crl_sources(
	sym: &vane_core::SymbolicFlowGraph,
) -> Vec<(CrlSourceId, CrlFetchFailure)> {
	use vane_core::FetchKind;
	let mut out = Vec::new();
	for sf in &sym.fetches {
		if !matches!(sf.kind, FetchKind::HttpProxy | FetchKind::WebSocketUpgrade) {
			continue;
		}
		let Some(arr) = sf.args.get("tls").and_then(|t| t.get("crls")).and_then(|v| v.as_array())
		else {
			continue;
		};
		for entry in arr {
			if let Ok(cfg) = serde_json::from_value::<vane_core::rule::CrlSourceConfig>(entry.clone()) {
				out.push(crate::tls::client_trust::crl_source_from_config(&cfg));
			}
		}
	}
	out
}

/// Walk every listener TLS spec and collect its CRL source list.
#[must_use]
pub fn collect_listener_crl_sources(
	listener_tls: &std::collections::BTreeMap<std::net::SocketAddr, vane_core::rule::ListenerTlsSpec>,
) -> Vec<(CrlSourceId, CrlFetchFailure)> {
	use vane_core::rule::ClientAuthSpec;
	let mut out = Vec::new();
	for spec in listener_tls.values() {
		let trust_store = match &spec.client_auth {
			ClientAuthSpec::None => continue,
			ClientAuthSpec::Request { trust_store } | ClientAuthSpec::Require { trust_store } => {
				trust_store
			}
		};
		for cfg in &trust_store.crls {
			out.push(crate::tls::client_trust::crl_source_from_config(cfg));
		}
	}
	out
}
