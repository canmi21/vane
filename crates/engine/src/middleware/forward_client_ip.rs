//! `forward_client_ip` — inject the L4 peer's IP into the outgoing
//! request as `X-Forwarded-For` (append) and / or `X-Real-IP`
//! (overwrite).
//!
//! Off by default at the raw-rule layer. The `reverse_proxy` preset
//! (`spec/crates/core.md`) inserts this middleware
//! automatically with the default header set.
//!
//! Header semantics (per `spec/crates/engine.md` § _Middleware_):
//!
//! - `X-Forwarded-For` — append. If the request already carries one, the
//!   client IP is appended after a `", "` separator so the chain is
//!   preserved (`upstream-proxy.ip, our-client.ip`). If the existing
//!   value is non-ASCII (so `to_str` fails), it is overwritten cleanly.
//! - `X-Real-IP` — overwrite. Always set to the L4 peer; an upstream
//!   proxy's claim is intentionally clobbered because the daemon's L4
//!   peer is the authoritative observation.
//!
//! Always returns `Decision::Continue` — this middleware never short-
//! circuits.
//!

use std::sync::Arc;

use async_trait::async_trait;
use vane_core::{
	ConnContext, Decision, Error, FlowCtx, L7RequestMiddleware, MiddlewareKind, Request,
};

use crate::factories::{FactoryError, MiddlewareFactories};
use crate::flow_graph::MiddlewareInst;

/// Per-header injection strategy. Decided at factory time so `run()` is
/// a flat dispatch on the enum tag, not a string compare per request.
enum HeaderAction {
	XForwardedForAppend,
	XRealIpOverwrite,
}

pub struct ForwardClientIp {
	actions: Vec<HeaderAction>,
}

#[async_trait]
impl L7RequestMiddleware for ForwardClientIp {
	async fn run(
		&self,
		req: &mut Request,
		conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<Decision, Error> {
		// `SocketAddr::ip().to_string()` renders IPv4 dotted-decimal and
		// IPv6 in RFC 5952 canonical form — both are valid header value
		// bytes (visible ASCII), so HeaderValue::from_str cannot fail and
		// the .expect below is justified.
		let client_ip = conn.remote.ip().to_string();
		for action in &self.actions {
			match action {
				HeaderAction::XForwardedForAppend => {
					let name = http::header::HeaderName::from_static("x-forwarded-for");
					let new_value = match req.headers().get(&name).and_then(|v| v.to_str().ok()) {
						Some(existing) => format!("{existing}, {client_ip}"),
						None => client_ip.clone(),
					};
					let value =
						http::HeaderValue::from_str(&new_value).expect("ip-derived header value is ascii");
					req.headers_mut().insert(name, value);
				}
				HeaderAction::XRealIpOverwrite => {
					let name = http::header::HeaderName::from_static("x-real-ip");
					let value =
						http::HeaderValue::from_str(&client_ip).expect("ip-derived header value is ascii");
					req.headers_mut().insert(name, value);
				}
			}
		}
		Ok(Decision::Continue)
	}
}

/// Args parser exposed as a registry-friendly factory.
///
/// Args shape (all fields optional):
///
/// ```json
/// { "headers": ["x-forwarded-for", "x-real-ip"] }
/// ```
///
/// `headers` defaults to `["x-forwarded-for", "x-real-ip"]`. Currently
/// only those two header names are recognised — anything else is
/// rejected with a pointed error so a typo (`"x-forwared-for"`) doesn't
/// silently disable the injection. Wider support lands when there is a
/// concrete need.
///
/// # Errors
/// Returns [`FactoryError`] when `headers` is not an array, contains
/// non-string elements, or names a header outside the supported set.
pub fn factory(args: &serde_json::Value) -> Result<MiddlewareInst, FactoryError> {
	let actions = match args.get("headers") {
		Some(value) => {
			let arr = value
				.as_array()
				.ok_or_else(|| FactoryError("args.headers must be an array".to_string()))?;
			let mut out = Vec::with_capacity(arr.len());
			for item in arr {
				let s = item
					.as_str()
					.ok_or_else(|| FactoryError("args.headers items must be strings".to_string()))?;
				let lower = s.to_ascii_lowercase();
				let action = match lower.as_str() {
					"x-forwarded-for" => HeaderAction::XForwardedForAppend,
					"x-real-ip" => HeaderAction::XRealIpOverwrite,
					_ => {
						return Err(FactoryError(format!(
							"unsupported header {s:?}; supported: x-forwarded-for, x-real-ip"
						)));
					}
				};
				out.push(action);
			}
			out
		}
		None => vec![HeaderAction::XForwardedForAppend, HeaderAction::XRealIpOverwrite],
	};
	Ok(MiddlewareInst::L7Request(Arc::new(ForwardClientIp { actions })))
}

/// Plug `forward_client_ip` into a `MiddlewareFactories` registry.
pub fn register(factories: &mut MiddlewareFactories) {
	factories.register("forward_client_ip", MiddlewareKind::L7Request, factory);
}
