//! WASM-runtime side `HttpFetchBackend` implementations.
//!
//! Plugins that call `host.http-fetch` go through whatever
//! [`vane_core::HttpFetchBackend`] the daemon plumbs into
//! `vane-wasm::WasmtimeRuntime` at boot. Two impls live here:
//!
//! * [`DenyAllHttpFetchBackend`] — fail-closed stub. Every call
//!   returns `NotAllowed`. Used by tests / builds that need a
//!   placeholder.
//! * [`HyperHttpFetchBackend`] — production. Hyper-util `legacy::Client`
//!   over a `hyper-rustls` connector with system roots and the
//!   hickory DNS resolver. Body cap, timeout, status mapping live
//!   here. The redirect-follow path is deferred — see
//!   `TODO(http-fetch-redirects)`.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use http_body_util::BodyExt as _;
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use vane_core::{
	Body, HttpFetchBackend, HttpFetchError, HttpFetchLimits, HttpFetchRequest, HttpFetchResponse,
};

use crate::fetch::dns::{DnsConfig, HickoryDnsResolver};
use crate::fetch::upstream;

/// Stub `HttpFetchBackend` whose every call returns
/// [`HttpFetchError::NotAllowed`]. The error message names this stub
/// explicitly so operators reading the structured log understand
/// _why_ the call failed (vs. e.g. policy or DNS).
pub struct DenyAllHttpFetchBackend;

#[async_trait]
impl HttpFetchBackend for DenyAllHttpFetchBackend {
	async fn fetch(
		&self,
		_req: HttpFetchRequest,
		_limits: HttpFetchLimits,
	) -> Result<HttpFetchResponse, HttpFetchError> {
		Err(HttpFetchError::NotAllowed(
			"http-fetch backend not yet wired in this daemon build (DenyAllHttpFetchBackend)".to_string(),
		))
	}
}

/// Production `HttpFetchBackend` for plugin `host.http-fetch`. One
/// `hyper-util` `legacy::Client` over a `hyper-rustls` connector
/// constructed once and shared by every plugin call. System trust
/// store, ALPN `h2` + `http/1.1`, system DNS via the engine's
/// `HickoryDnsResolver`. Per-request limits (`max_body_bytes`,
/// `timeout_ms`) are enforced here; allowed-hosts and TLS-skip
/// gates fire before the call reaches us in
/// `vane_wasm::http_fetch_core`.
//
// TODO(http-fetch-redirects): hyper-util's legacy client does not
// follow redirects. Plugins receive the raw 3xx response. A
// follow-up adds a manual loop bounded by `limits.follow_redirects`
// — outside this PR's scope to keep the diff bounded.
pub struct HyperHttpFetchBackend {
	client: Client<HttpsConnector<HttpConnector<HickoryDnsResolver>>, Body>,
}

impl HyperHttpFetchBackend {
	/// Build the shared client. Fails when the system trust store
	/// fails to load or the DNS resolver fails to construct — both
	/// only happen on broken host configuration.
	///
	/// # Errors
	/// Returns `String` with the underlying failure cause.
	pub fn new() -> Result<Self, String> {
		let tls_cfg = upstream::build_client_config(false)?;
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
		let client = hyper_util::client::legacy::Client::builder(TokioExecutor::new()).build(https);
		Ok(Self { client })
	}

	/// Construct an `Arc`-shared backend in one step — daemons that
	/// inject `Arc<dyn HttpFetchBackend>` into `WasmtimeRuntime`
	/// don't need the bare value.
	///
	/// # Errors
	/// As [`HyperHttpFetchBackend::new`].
	pub fn new_arc() -> Result<Arc<Self>, String> {
		Ok(Arc::new(Self::new()?))
	}
}

#[async_trait]
impl HttpFetchBackend for HyperHttpFetchBackend {
	async fn fetch(
		&self,
		req: HttpFetchRequest,
		limits: HttpFetchLimits,
	) -> Result<HttpFetchResponse, HttpFetchError> {
		let mut builder = http::Request::builder().method(req.method.as_str()).uri(&req.url);
		for (k, v) in &req.headers {
			builder = builder.header(k, v);
		}
		let body = Body::Static(bytes::Bytes::from(req.body));
		let hyper_req =
			builder.body(body).map_err(|e| HttpFetchError::Internal(format!("build request: {e}")))?;

		let timeout = Duration::from_millis(u64::from(limits.timeout_ms.unwrap_or(30_000)));
		let resp = match tokio::time::timeout(timeout, self.client.request(hyper_req)).await {
			Ok(Ok(r)) => r,
			Ok(Err(e)) => return Err(map_hyper_error(&e)),
			Err(_) => return Err(HttpFetchError::Timeout),
		};

		let (parts, body) = resp.into_parts();
		let max_bytes = usize::try_from(limits.max_body_bytes).unwrap_or(usize::MAX);
		let limited = http_body_util::Limited::new(body, max_bytes);
		let bytes = match limited.collect().await {
			Ok(c) => c.to_bytes().to_vec(),
			Err(e) => {
				if e.is::<http_body_util::LengthLimitError>() {
					return Err(HttpFetchError::BodyTooLarge);
				}
				return Err(HttpFetchError::Internal(format!("collect body: {e}")));
			}
		};

		let headers: Vec<(String, String)> = parts
			.headers
			.iter()
			.filter_map(|(k, v)| v.to_str().ok().map(|vs| (k.to_string(), vs.to_string())))
			.collect();
		Ok(HttpFetchResponse { status: parts.status.as_u16(), headers, body: bytes })
	}
}

/// hyper-util's `legacy::Error` doesn't expose a typed cause, so we
/// best-effort match on the message to surface DNS / connection /
/// TLS failures separately. Everything else collapses to `Internal`.
fn map_hyper_error(e: &hyper_util::client::legacy::Error) -> HttpFetchError {
	let s = e.to_string().to_lowercase();
	if s.contains("dns") || s.contains("name resolution") || s.contains("nodename nor servname") {
		HttpFetchError::DnsFailure(e.to_string())
	} else if s.contains("connection refused") {
		HttpFetchError::ConnectionRefused
	} else if s.contains("tls") || s.contains("certificate") {
		HttpFetchError::TlsError(e.to_string())
	} else {
		HttpFetchError::Internal(e.to_string())
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn deny_all_returns_not_allowed_with_descriptive_message() {
		let backend = DenyAllHttpFetchBackend;
		let req = HttpFetchRequest {
			method: "GET".to_string(),
			url: "https://example.invalid/".to_string(),
			headers: Vec::new(),
			body: Vec::new(),
			timeout_ms: None,
			follow_redirects: None,
			verify_tls: None,
		};
		let err =
			backend.fetch(req, HttpFetchLimits::default()).await.expect_err("DenyAll must reject");
		match err {
			HttpFetchError::NotAllowed(msg) => {
				assert!(
					msg.contains("DenyAllHttpFetchBackend"),
					"error must self-identify the stub: {msg}",
				);
			}
			other => panic!("expected NotAllowed, got {other:?}"),
		}
	}
}
