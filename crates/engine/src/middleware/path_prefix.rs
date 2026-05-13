//! `path_prefix` — accept requests whose URI path starts with one of the
//! configured prefixes; short-circuit-close the rest.
//!
//! Strict byte-prefix match — RFC 3986 path is case-sensitive (unlike
//! scheme/authority), so `/Api` and `/api` are distinct. Use the bare
//! `"/"` prefix to match every path.
//!
//! See `spec/crates/engine.md` § _Middleware_.

use std::borrow::Cow;
use std::sync::Arc;

use async_trait::async_trait;
use vane_core::{
	CloseReason, ConnContext, Decision, Error, FlowCtx, L7RequestMiddleware, MiddlewareKind, Request,
	ShortCircuit,
};

use crate::factories::{FactoryError, MiddlewareFactories};
use crate::flow_graph::MiddlewareInst;

pub struct PathPrefix {
	prefixes: Vec<Arc<str>>,
}

#[async_trait]
impl L7RequestMiddleware for PathPrefix {
	async fn run(
		&self,
		req: &mut Request,
		_conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<Decision, Error> {
		let path = req.uri().path();
		let matched = self.prefixes.iter().any(|p| path.starts_with(p.as_ref()));
		if matched {
			Ok(Decision::Continue)
		} else {
			Ok(Decision::Short(ShortCircuit::Close(CloseReason::PolicyDenied(Cow::Borrowed(
				"path_prefix: no prefix matched",
			)))))
		}
	}
}

/// Args parser exposed as a registry-friendly factory.
///
/// Args shape:
///
/// ```json
/// { "prefixes": ["/api", "/admin"] }
/// ```
///
/// `prefixes` is required and must be a non-empty array of strings.
///
/// # Errors
/// Returns [`FactoryError`] when `prefixes` is missing, not an array,
/// empty, or contains non-string elements.
pub fn factory(args: &serde_json::Value) -> Result<MiddlewareInst, FactoryError> {
	let arr = args.get("prefixes").and_then(serde_json::Value::as_array).ok_or_else(|| {
		FactoryError::Invalid("missing args.prefixes (non-empty string array)".to_string())
	})?;
	if arr.is_empty() {
		return Err(FactoryError::Invalid(
			"args.prefixes must contain at least one prefix".to_string(),
		));
	}
	let mut prefixes = Vec::with_capacity(arr.len());
	for item in arr {
		let s = item
			.as_str()
			.ok_or_else(|| FactoryError::Invalid("args.prefixes items must be strings".to_string()))?;
		prefixes.push(Arc::from(s));
	}
	Ok(MiddlewareInst::L7Request(Arc::new(PathPrefix { prefixes })))
}

/// Plug `path_prefix` into a `MiddlewareFactories` registry.
pub fn register(factories: &mut MiddlewareFactories) {
	factories.register("path_prefix", MiddlewareKind::L7Request, factory);
}
