//! `sni_peek` `L4Peek` middleware. A phase-advancement marker: the
//! listener's peek prelude (see `crates/engine/src/listener.rs`)
//! already buffers the connection prefix, parses any TLS
//! `ClientHello` via `crates/engine/src/protocol_detect.rs::parse_client_hello`,
//! and writes `ConnContext.tls.sni` lowercase before any middleware
//! fires. This middleware exists so a rule can declare "I read
//! `tls.sni` here": the declaration is what `flow_graph::needs_peek`
//! looks for to gate the listener onto the peek branch, and it
//! advances the executor's phase from `L4Raw` to `L4Peeked` so the
//! `tls.sni` / `tls.alpn` predicates that follow are legal at this
//! point in the graph.
//!
//! See `spec/crates/engine.md` § _Protocol detection_ for the
//! full pipeline; this file is the rule-facing handle.

use std::sync::Arc;

use async_trait::async_trait;
use vane_core::{ConnContext, Decision, Error, FlowCtx, L4PeekMiddleware, MiddlewareKind};

use crate::factories::{FactoryError, MiddlewareFactories};
use crate::flow_graph::MiddlewareInst;

#[derive(Debug, Default)]
pub struct SniPeekMiddleware;

#[async_trait]
impl L4PeekMiddleware for SniPeekMiddleware {
	async fn run(
		&self,
		_peek: &[u8],
		_conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<Decision, Error> {
		// Nop. The listener prelude already populated `conn.tls.sni`
		// from the parsed `ClientHello`; running the work again here
		// would only re-derive the same bytes.
		Ok(Decision::Continue)
	}
}

/// Plug `sni_peek` into a `MiddlewareFactories` registry.
pub fn register(factories: &mut MiddlewareFactories) {
	factories.register("sni_peek", MiddlewareKind::L4Peek, factory);
}

/// `sni_peek` takes no configuration. Accepts `null` or `{}`; any
/// other shape — fields, arrays, scalars — is a typo we'd rather
/// catch at link than at the first connection.
///
/// # Errors
/// Returns [`FactoryError`] when `args` carries any field, or is
/// neither a JSON object nor `null`.
fn factory(args: &serde_json::Value) -> Result<MiddlewareInst, FactoryError> {
	match args {
		serde_json::Value::Null => {}
		serde_json::Value::Object(map) if map.is_empty() => {}
		serde_json::Value::Object(map) => {
			let unexpected: Vec<&str> = map.keys().map(String::as_str).collect();
			return Err(FactoryError::Invalid(format!(
				"sni_peek: unexpected field(s): {}",
				unexpected.join(", "),
			)));
		}
		other => {
			return Err(FactoryError::Invalid(format!(
				"sni_peek: args must be null or an empty object, got {}",
				type_name(other),
			)));
		}
	}
	Ok(MiddlewareInst::L4Peek(Arc::new(SniPeekMiddleware)))
}

fn type_name(v: &serde_json::Value) -> &'static str {
	match v {
		serde_json::Value::Null => "null",
		serde_json::Value::Bool(_) => "bool",
		serde_json::Value::Number(_) => "number",
		serde_json::Value::String(_) => "string",
		serde_json::Value::Array(_) => "array",
		serde_json::Value::Object(_) => "object",
	}
}

#[cfg(test)]
mod tests {
	use std::net::SocketAddr;
	use std::sync::Arc;
	use std::time::Instant;

	use parking_lot::Mutex;
	use serde_json::json;
	use tokio_util::sync::CancellationToken;
	use vane_core::{
		ConnContext, ConnId, Decision, FlowCtx, FlowLogEvent, FlowLogSink, FlowLogVerbosity,
		L4PeekMiddleware, NodeId, TrajectoryBuilder, Transport,
	};

	use super::*;

	struct NullSink;
	impl FlowLogSink for NullSink {
		fn emit(&self, _event: FlowLogEvent) {}
	}

	fn make_conn() -> Arc<ConnContext> {
		let addr: SocketAddr = "127.0.0.1:0".parse().expect("parse addr");
		Arc::new(ConnContext {
			id: ConnId(0),
			remote: addr,
			local: addr,
			transport: Transport::Tcp,
			entered_at: Instant::now(),
			tls: Mutex::new(None),
			http_version: std::sync::OnceLock::new(),
			user: Mutex::new(http::Extensions::new()),
		})
	}

	fn make_ctx() -> FlowCtx {
		FlowCtx {
			span: tracing::Span::none(),
			log: Arc::new(NullSink),
			cancel: CancellationToken::new(),
			accept_cancel: CancellationToken::new(),
			verbosity: FlowLogVerbosity::Trajectory,
			trajectory: TrajectoryBuilder::new(ConnId(0), NodeId::new(0), 0),
		}
	}

	#[tokio::test]
	async fn run_returns_continue_regardless_of_peek_bytes() {
		let mw = SniPeekMiddleware;
		let conn = make_conn();
		let mut ctx = make_ctx();
		let res = mw.run(b"", &conn, &mut ctx).await.expect("run ok");
		assert!(matches!(res, Decision::Continue));

		let res = mw.run(&[0x16, 0x03, 0x01, 0xff], &conn, &mut ctx).await.expect("run ok");
		assert!(matches!(res, Decision::Continue));
	}

	#[test]
	fn factory_accepts_null() {
		factory(&serde_json::Value::Null).expect("null args accepted");
	}

	#[test]
	fn factory_accepts_empty_object() {
		factory(&json!({})).expect("empty object accepted");
	}

	#[test]
	fn factory_rejects_unknown_field() {
		// `MiddlewareInst` does not implement `Debug`, so let-else is the
		// destructure strategy (mirrors the convention in
		// `crates/engine/tests/fetch_http_synthesize.rs`).
		let Err(FactoryError::Invalid(msg)) = factory(&json!({ "foo": 1 })) else {
			panic!("unknown field must be rejected");
		};
		assert!(msg.contains("foo"), "{msg}");
	}

	#[test]
	fn factory_rejects_non_object_args() {
		let Err(FactoryError::Invalid(msg)) = factory(&json!([])) else {
			panic!("array args must be rejected");
		};
		assert!(msg.contains("sni_peek"), "{msg}");
	}
}
