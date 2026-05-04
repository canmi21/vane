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
//!   over a `hyper-rustls` connector, paired with a sibling client
//!   over a `NoVerify` TLS connector for the operator-opted-in
//!   insecure path. Body cap, timeout, redirect-follow, status mapping
//!   live here.

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

type HyperClient = Client<HttpsConnector<HttpConnector<HickoryDnsResolver>>, Body>;

/// Production `HttpFetchBackend` for plugin `host.http-fetch`. Two
/// `hyper-util` `legacy::Client`s constructed once and shared by every
/// plugin call:
///
/// * `client` — system trust store, full TLS verification. Used
///   whenever `limits.allow_insecure == false` (the default path).
/// * `client_insecure` — `NoVerify` certificate verifier. Used only
///   when the operator policy says `allow_insecure: true` AND the
///   plugin's per-call `verify_tls: false` arrives — the host-fn layer
///   composes those two gates into `limits.allow_insecure` before
///   reaching us.
///
/// Both clients share ALPN `h2` + `http/1.1` and the engine's hickory
/// DNS resolver. Per-request limits (`max_body_bytes`, `timeout_ms`,
/// `follow_redirects`) are enforced here.
pub struct HyperHttpFetchBackend {
	client: HyperClient,
	client_insecure: HyperClient,
}

impl HyperHttpFetchBackend {
	/// Build the shared clients. Fails when the system trust store
	/// fails to load or the DNS resolver fails to construct — both
	/// only happen on broken host configuration.
	///
	/// # Errors
	/// Returns `String` with the underlying failure cause.
	pub fn new() -> Result<Self, String> {
		Ok(Self { client: build_client(false)?, client_insecure: build_client(true)? })
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

fn build_client(insecure: bool) -> Result<HyperClient, String> {
	let tls_cfg = upstream::build_client_config(insecure)?;
	let resolver =
		HickoryDnsResolver::build(&DnsConfig::System).map_err(|e| format!("hickory resolver: {e}"))?;
	let mut http = HttpConnector::new_with_resolver(resolver);
	http.enforce_http(false);
	let https = hyper_rustls::HttpsConnectorBuilder::new()
		.with_tls_config((*tls_cfg).clone())
		.https_or_http()
		.enable_http1()
		.enable_http2()
		.wrap_connector(http);
	Ok(hyper_util::client::legacy::Client::builder(TokioExecutor::new()).build(https))
}

#[async_trait]
impl HttpFetchBackend for HyperHttpFetchBackend {
	async fn fetch(
		&self,
		req: HttpFetchRequest,
		limits: HttpFetchLimits,
	) -> Result<HttpFetchResponse, HttpFetchError> {
		// Pick the verified or insecure client per the host-fn-resolved
		// gate. `limits.allow_insecure == true` is only set when both
		// the operator policy permits and the per-call request opted
		// out — see `vane_wasm::http_fetch_core`.
		let client = if limits.allow_insecure { &self.client_insecure } else { &self.client };

		let max_redirects = limits.follow_redirects.unwrap_or(5);
		let timeout = Duration::from_millis(u64::from(limits.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS)));
		let max_body_bytes = usize::try_from(limits.max_body_bytes).unwrap_or(usize::MAX);

		let mut current_url = req.url;
		let mut current_method = req.method;
		let mut current_body = req.body;
		// Headers are preserved verbatim across redirects. Cross-host
		// `Authorization` stripping is a future hardening — none of
		// today's allowed_hosts policies rely on per-host scoping.
		let headers = req.headers;

		let mut hops: u32 = 0;
		loop {
			let resp = dispatch_one(
				client,
				&current_method,
				&current_url,
				&headers,
				&current_body,
				timeout,
				max_body_bytes,
			)
			.await?;

			if !is_redirect(resp.status) || hops >= max_redirects {
				return Ok(resp);
			}

			// `Location` header is mandatory on a redirect we're going
			// to follow; absence means the upstream is malformed and
			// we hand the response back so the plugin can decide.
			let location = match resp.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("location")) {
				Some((_, v)) => v.clone(),
				None => return Ok(resp),
			};
			let next_url = resolve_url(&current_url, &location)?;

			// RFC 7231 / RFC 7538 method-rewrite rules. 301/302/303
			// downgrade to GET (except HEAD, which stays HEAD); 307/308
			// preserve the method and body verbatim.
			let next_method = match resp.status {
				301..=303 => {
					if current_method.eq_ignore_ascii_case("HEAD") {
						current_method
					} else {
						"GET".to_string()
					}
				}
				307 | 308 => current_method,
				_ => unreachable!("is_redirect already filtered the status"),
			};
			// GET / HEAD never carry a body across the redirect.
			let next_body =
				if next_method.eq_ignore_ascii_case("GET") || next_method.eq_ignore_ascii_case("HEAD") {
					Vec::new()
				} else {
					current_body
				};

			tracing::debug!(
				target: "vane::wasm::http_fetch",
				from = %current_url,
				to = %next_url,
				status = resp.status,
				hop = hops + 1,
				"following redirect",
			);

			current_url = next_url;
			current_method = next_method;
			current_body = next_body;
			hops += 1;
		}
	}
}

const DEFAULT_TIMEOUT_MS: u32 = 30_000;

async fn dispatch_one(
	client: &HyperClient,
	method: &str,
	url: &str,
	headers: &[(String, String)],
	body: &[u8],
	timeout: Duration,
	max_body_bytes: usize,
) -> Result<HttpFetchResponse, HttpFetchError> {
	let mut builder = http::Request::builder().method(method).uri(url);
	for (k, v) in headers {
		builder = builder.header(k, v);
	}
	let hyper_body = Body::Static(bytes::Bytes::copy_from_slice(body));
	let hyper_req = builder
		.body(hyper_body)
		.map_err(|e| HttpFetchError::Internal(format!("build request: {e}")))?;

	let resp = match tokio::time::timeout(timeout, client.request(hyper_req)).await {
		Ok(Ok(r)) => r,
		Ok(Err(e)) => return Err(map_hyper_error(&e)),
		Err(_) => return Err(HttpFetchError::Timeout),
	};

	let (parts, resp_body) = resp.into_parts();
	let limited = http_body_util::Limited::new(resp_body, max_body_bytes);
	let bytes = match limited.collect().await {
		Ok(c) => c.to_bytes().to_vec(),
		Err(e) => {
			if e.is::<http_body_util::LengthLimitError>() {
				return Err(HttpFetchError::BodyTooLarge);
			}
			return Err(HttpFetchError::Internal(format!("collect body: {e}")));
		}
	};

	let resp_headers: Vec<(String, String)> = parts
		.headers
		.iter()
		.filter_map(|(k, v)| v.to_str().ok().map(|vs| (k.to_string(), vs.to_string())))
		.collect();
	Ok(HttpFetchResponse { status: parts.status.as_u16(), headers: resp_headers, body: bytes })
}

fn is_redirect(status: u16) -> bool {
	matches!(status, 301 | 302 | 303 | 307 | 308)
}

fn resolve_url(base: &str, location: &str) -> Result<String, HttpFetchError> {
	let base_url = url::Url::parse(base)
		.map_err(|e| HttpFetchError::Internal(format!("redirect base url parse: {e}")))?;
	let resolved = base_url
		.join(location)
		.map_err(|e| HttpFetchError::Internal(format!("redirect target resolve: {e}")))?;
	Ok(resolved.to_string())
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

	#[test]
	fn resolve_url_joins_relative_path_against_base() {
		assert_eq!(
			resolve_url("https://a.example/foo/bar", "/baz").expect("resolve"),
			"https://a.example/baz",
		);
		assert_eq!(
			resolve_url("https://a.example/foo/bar", "qux").expect("resolve"),
			"https://a.example/foo/qux",
		);
		assert_eq!(
			resolve_url("https://a.example/foo/bar", "https://b.example/elsewhere")
				.expect("resolve absolute"),
			"https://b.example/elsewhere",
		);
	}

	#[test]
	fn is_redirect_classifies_only_explicit_status_codes() {
		for s in [301, 302, 303, 307, 308] {
			assert!(is_redirect(s), "{s} must redirect");
		}
		for s in [200, 300, 304, 400, 500] {
			assert!(!is_redirect(s), "{s} must not redirect");
		}
	}
}
