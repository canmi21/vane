//! WASM-runtime side `HttpFetchBackend` implementations.
//!
//! Plugins that call `host.http-fetch` go through whatever
//! [`vane_core::HttpFetchBackend`] the daemon plumbs into
//! `vane-wasm::WasmtimeRuntime` at boot. The real implementation —
//! TLS-aware, pool-backed, policy-bounded per
//! `spec/architecture/11-wasm.md` § _http-fetch policy_ — is a
//! separate piece of work; until then daemons that load WASM plugins
//! pair the runtime with [`DenyAllHttpFetchBackend`] so plugins can
//! still execute, but `http-fetch` calls return a typed
//! [`HttpFetchError::NotAllowed`] that surfaces in the structured log.

use async_trait::async_trait;
use vane_core::{
	HttpFetchBackend, HttpFetchError, HttpFetchLimits, HttpFetchRequest, HttpFetchResponse,
};

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
