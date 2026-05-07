//! `host_header_match` — accept requests whose `Host` header matches one
//! of the configured authorities; short-circuit-close the rest.
//!
//! Comparison is ASCII-case-insensitive — the factory pre-lowercases the
//! configured host list and the runtime lowercases the incoming header
//! once per call. Port-bearing values (`example.com:8443`) are compared
//! verbatim; rules that want to ignore the port should configure both
//! the bare host and the `host:port` form, or normalise upstream.
//!
//! See `spec/crates/engine.md` § _Stateless internal_.

use std::borrow::Cow;
use std::sync::Arc;

use async_trait::async_trait;
use vane_core::{
	CloseReason, ConnContext, Decision, Error, FlowCtx, L7RequestMiddleware, MiddlewareKind, Request,
	ShortCircuit,
};

use crate::factories::{FactoryError, MiddlewareFactories};
use crate::flow_graph::MiddlewareInst;

pub struct HostHeaderMatch {
	/// Pre-lowercased authority list. `Arc<str>` keeps the per-request
	/// scan allocation-free; the wider list is small enough that linear
	/// scan beats hashing for the expected fan-out (single-digit hosts
	/// per rule).
	hosts: Vec<Arc<str>>,
}

#[async_trait]
impl L7RequestMiddleware for HostHeaderMatch {
	async fn run(
		&self,
		req: &mut Request,
		_conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<Decision, Error> {
		let matched = req
			.headers()
			.get(http::header::HOST)
			.and_then(|v| v.to_str().ok())
			.map(str::to_ascii_lowercase)
			.is_some_and(|h| self.hosts.iter().any(|expected| h == expected.as_ref()));
		if matched {
			Ok(Decision::Continue)
		} else {
			Ok(Decision::Short(ShortCircuit::Close(CloseReason::PolicyDenied(Cow::Borrowed(
				"host_header_match: no host matched",
			)))))
		}
	}
}

/// Args parser exposed as a registry-friendly factory.
///
/// Args shape:
///
/// ```json
/// { "hosts": ["api.example.com", "v2.example.com"] }
/// ```
///
/// `hosts` is required and must be a non-empty array of strings. Values
/// are ASCII-lowercased at factory time.
///
/// # Errors
/// Returns [`FactoryError`] when `hosts` is missing, not an array, empty,
/// or contains non-string elements.
pub fn factory(args: &serde_json::Value) -> Result<MiddlewareInst, FactoryError> {
	let arr = args
		.get("hosts")
		.and_then(serde_json::Value::as_array)
		.ok_or_else(|| FactoryError("missing args.hosts (non-empty string array)".to_string()))?;
	if arr.is_empty() {
		return Err(FactoryError("args.hosts must contain at least one host".to_string()));
	}
	let mut hosts = Vec::with_capacity(arr.len());
	for item in arr {
		let s =
			item.as_str().ok_or_else(|| FactoryError("args.hosts items must be strings".to_string()))?;
		hosts.push(Arc::from(s.to_ascii_lowercase()));
	}
	Ok(MiddlewareInst::L7Request(Arc::new(HostHeaderMatch { hosts })))
}

/// Plug `host_header_match` into a `MiddlewareFactories` registry.
pub fn register(factories: &mut MiddlewareFactories) {
	factories.register("host_header_match", MiddlewareKind::L7Request, factory);
}
