//! `HttpSynthesizeFetch` — fabricate an in-memory response.
//!
//! Used for redirects, "maintenance" pages, default-deny responses, and
//! trivial health checks — anywhere a rule wants to answer without
//! contacting an upstream. Always returns `Body::Static` (or
//! `Body::Empty` for an empty payload), per 07-l7.md
//! § _`HttpProxyFetch` commits to streaming response bodies_:
//! "`HttpSynthesizeFetch` always produces `Body::Static` by construction
//! — that is the point of synthesis."
//!
//! See `spec/architecture/05-terminator.md` § _`HttpSynthesize`_.
//! Feature: S1-20.

use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use bytes::Bytes;
use http::HeaderName;
use vane_core::{Body, ConnContext, Error, FetchKind, FlowCtx, L7Fetch, L7FetchOutput, Request};

use crate::factories::{FactoryError, FetchFactories};
use crate::flow_graph::FetchInst;

/// Fixed-payload L7 fetch. Status, headers, and body are resolved at
/// factory time so the per-request work is just response construction.
pub struct HttpSynthesizeFetch {
	status: u16,
	/// Header name+value pairs in factory-declaration order. Names are
	/// pre-validated as `HeaderName`; values are validated lazily by
	/// `http::response::Builder::header` per request — invalid bytes
	/// surface as a build error.
	headers: Vec<(HeaderName, String)>,
	/// Empty `Bytes` → `Body::Empty`; non-empty → `Body::Static(b)`.
	body: Bytes,
}

#[async_trait]
impl L7Fetch for HttpSynthesizeFetch {
	async fn fetch(
		&self,
		_req: Request,
		_conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<L7FetchOutput, Error> {
		let mut builder = http::Response::builder().status(self.status);
		for (name, value) in &self.headers {
			builder = builder.header(name, value);
		}
		let body = if self.body.is_empty() { Body::Empty } else { Body::Static(self.body.clone()) };
		let resp =
			builder.body(body).map_err(|e| Error::internal(format!("synth response build: {e}")))?;
		Ok(L7FetchOutput::Response(resp))
	}
}

/// Args parser exposed as a registry-friendly factory.
///
/// Args shape:
///
/// ```json
/// {
///   "status":  200,
///   "headers": { "content-type": "text/plain", "x-via": "vane" },
///   "body":    "aGVsbG8="
/// }
/// ```
///
/// `status` is required (HTTP integer in `100..=599`). `headers` is
/// optional (string-only values). `body` is optional, base64-encoded
/// raw bytes — JSON has no native byte type, so the preset expansion
/// pass (S1-22) is responsible for translating user-friendly text into
/// base64 before reaching this factory.
///
/// # Errors
/// Returns [`FactoryError`] for any of: missing/non-integer/out-of-range
/// status, invalid header name, non-string header value, or malformed
/// base64 body.
pub fn factory(args: &serde_json::Value) -> Result<FetchInst, FactoryError> {
	let status_raw = args
		.get("status")
		.and_then(serde_json::Value::as_u64)
		.ok_or_else(|| FactoryError("missing args.status (integer 100-599)".to_string()))?;
	let status = u16::try_from(status_raw)
		.map_err(|_| FactoryError(format!("status {status_raw} out of u16 range")))?;
	if !(100..=599).contains(&status) {
		return Err(FactoryError(format!("status {status} out of HTTP range 100-599")));
	}

	let mut headers = Vec::new();
	if let Some(obj) = args.get("headers").and_then(serde_json::Value::as_object) {
		for (k, v) in obj {
			let name = HeaderName::try_from(k.as_str())
				.map_err(|e| FactoryError(format!("invalid header name {k:?}: {e}")))?;
			let value =
				v.as_str().ok_or_else(|| FactoryError(format!("header {k:?} value must be string")))?;
			headers.push((name, value.to_string()));
		}
	}

	let body = if let Some(b64) = args.get("body").and_then(serde_json::Value::as_str) {
		Bytes::from(
			BASE64_STANDARD
				.decode(b64.as_bytes())
				.map_err(|e| FactoryError(format!("args.body base64 decode: {e}")))?,
		)
	} else {
		Bytes::new()
	};

	Ok(FetchInst::L7(Arc::new(HttpSynthesizeFetch { status, headers, body })))
}

/// Plug `FetchKind::HttpSynthesize` into a `FetchFactories` registry.
pub fn register(factories: &mut FetchFactories) {
	factories.register(FetchKind::HttpSynthesize, factory);
}
