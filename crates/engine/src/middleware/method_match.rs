//! `method_match` — accept requests whose method is on the configured
//! allow-list; short-circuit-close the rest.
//!
//! Method tokens are validated through [`http::Method::from_bytes`] at
//! factory time. Inputs are uppercased before parsing so `["get"]` and
//! `["GET"]` both produce the same allow-list, but anything containing
//! whitespace, control bytes, or other RFC-7230-illegal characters is
//! rejected before the rule reaches runtime.
//!
//! See `spec/architecture/04-middleware.md` § _Stateless internal_.
//! Feature: S1-21.

use std::borrow::Cow;
use std::sync::Arc;

use async_trait::async_trait;
use vane_core::{
	CloseReason, ConnContext, Decision, Error, FlowCtx, L7RequestMiddleware, MiddlewareKind, Request,
	ShortCircuit,
};

use crate::factories::{FactoryError, MiddlewareFactories};
use crate::flow_graph::MiddlewareInst;

pub struct MethodMatch {
	methods: Vec<http::Method>,
}

#[async_trait]
impl L7RequestMiddleware for MethodMatch {
	async fn run(
		&self,
		req: &mut Request,
		_conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<Decision, Error> {
		let method = req.method();
		let matched = self.methods.iter().any(|m| m == method);
		if matched {
			Ok(Decision::Continue)
		} else {
			Ok(Decision::Short(ShortCircuit::Close(CloseReason::PolicyDenied(Cow::Borrowed(
				"method_match: method not allowed",
			)))))
		}
	}
}

/// Args parser exposed as a registry-friendly factory.
///
/// Args shape:
///
/// ```json
/// { "methods": ["GET", "POST"] }
/// ```
///
/// `methods` is required and must be a non-empty array of strings. Each
/// entry is uppercased then parsed via [`http::Method::from_bytes`];
/// invalid tokens fail the build.
///
/// # Errors
/// Returns [`FactoryError`] when `methods` is missing, not an array,
/// empty, contains non-string elements, or contains a token that
/// `http::Method` rejects.
pub fn factory(args: &serde_json::Value) -> Result<MiddlewareInst, FactoryError> {
	let arr = args
		.get("methods")
		.and_then(serde_json::Value::as_array)
		.ok_or_else(|| FactoryError("missing args.methods (non-empty string array)".to_string()))?;
	if arr.is_empty() {
		return Err(FactoryError("args.methods must contain at least one method".to_string()));
	}
	let mut methods = Vec::with_capacity(arr.len());
	for item in arr {
		let s = item
			.as_str()
			.ok_or_else(|| FactoryError("args.methods items must be strings".to_string()))?;
		let upper = s.to_ascii_uppercase();
		let parsed = http::Method::from_bytes(upper.as_bytes())
			.map_err(|e| FactoryError(format!("invalid method {s:?}: {e}")))?;
		methods.push(parsed);
	}
	Ok(MiddlewareInst::L7Request(Arc::new(MethodMatch { methods })))
}

/// Plug `method_match` into a `MiddlewareFactories` registry.
pub fn register(factories: &mut MiddlewareFactories) {
	factories.register("method_match", MiddlewareKind::L7Request, factory);
}
