use std::hash::Hash;
use std::sync::Arc;

use async_trait::async_trait;

use crate::body::{Request, Response};
use crate::conn_context::ConnContext;
use crate::error::Error;
use crate::flow_ctx::FlowCtx;
use crate::ir::NodeId;
use crate::l4::L4Conn;

#[async_trait]
pub trait L4PeekMiddleware: Send + Sync {
	async fn run(
		&self,
		peek: &[u8],
		conn: &Arc<ConnContext>,
		ctx: &mut FlowCtx,
	) -> Result<Decision, Error>;
}

#[async_trait]
pub trait L4BytesMiddleware: Send + Sync {
	async fn run(
		&self,
		l4: &mut L4Conn,
		conn: &Arc<ConnContext>,
		ctx: &mut FlowCtx,
	) -> Result<Decision, Error>;
}

#[async_trait]
pub trait L7RequestMiddleware: Send + Sync {
	async fn run(
		&self,
		req: &mut Request,
		conn: &Arc<ConnContext>,
		ctx: &mut FlowCtx,
	) -> Result<Decision, Error>;

	fn needs_body(&self) -> bool {
		false
	}
}

#[async_trait]
pub trait L7ResponseMiddleware: Send + Sync {
	async fn run(
		&self,
		resp: &mut Response,
		conn: &Arc<ConnContext>,
		ctx: &mut FlowCtx,
	) -> Result<Decision, Error>;

	fn needs_body(&self) -> bool {
		false
	}
}

pub enum Decision {
	Continue,
	Short(ShortCircuit),
}

pub enum ShortCircuit {
	Response(Response),
	Close(CloseReason),
}

#[derive(Clone, Debug)]
pub enum CloseReason {
	Graceful,
	PolicyDenied(std::borrow::Cow<'static, str>),
	ProtocolError(std::borrow::Cow<'static, str>),
	/// Daemon-initiated cancellation — listener `force_cancel` fired during
	/// shutdown drain (spec/topology.md § _Listener lifecycle_), or any other
	/// `ctx.cancel.cancelled()` propagation. Distinct from `Graceful` so
	/// management observers can distinguish "client EOF'd" from "daemon
	/// pulled the plug while in-flight."
	Cancelled,
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub enum MiddlewareKind {
	L4Peek,
	L4Bytes,
	L7Request,
	L7Response,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SymbolicMiddlewareRef {
	pub name: Arc<str>,
	pub args: serde_json::Value,
	pub kind: MiddlewareKind,
	pub stateless: bool,
	pub needs_body: bool,
	pub on_error: Option<NodeId>,
}

impl PartialEq for SymbolicMiddlewareRef {
	fn eq(&self, other: &Self) -> bool {
		self.name == other.name
			&& self.kind == other.kind
			&& self.stateless == other.stateless
			&& self.needs_body == other.needs_body
			&& self.on_error == other.on_error
			&& canonical_json_eq(&self.args, &other.args)
	}
}

impl Eq for SymbolicMiddlewareRef {}

impl std::hash::Hash for SymbolicMiddlewareRef {
	fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
		self.name.hash(state);
		self.kind.hash(state);
		self.stateless.hash(state);
		self.needs_body.hash(state);
		self.on_error.hash(state);
		hash_canonical_json(&self.args, state);
	}
}

fn canonical_json_eq(a: &serde_json::Value, b: &serde_json::Value) -> bool {
	use serde_json::Value;
	match (a, b) {
		(Value::Null, Value::Null) => true,
		(Value::Bool(x), Value::Bool(y)) => x == y,
		(Value::Number(x), Value::Number(y)) => x == y,
		(Value::String(x), Value::String(y)) => x == y,
		(Value::Array(xs), Value::Array(ys)) => {
			xs.len() == ys.len() && xs.iter().zip(ys).all(|(x, y)| canonical_json_eq(x, y))
		}
		(Value::Object(xs), Value::Object(ys)) if xs.len() == ys.len() => {
			xs.iter().all(|(k, v)| ys.get(k).is_some_and(|w| canonical_json_eq(v, w)))
		}
		_ => false,
	}
}

fn hash_canonical_json<H: std::hash::Hasher>(v: &serde_json::Value, state: &mut H) {
	use serde_json::Value;
	match v {
		Value::Null => 0u8.hash(state),
		Value::Bool(b) => {
			1u8.hash(state);
			b.hash(state);
		}
		Value::Number(n) => {
			2u8.hash(state);
			n.to_string().hash(state);
		}
		Value::String(s) => {
			3u8.hash(state);
			s.hash(state);
		}
		Value::Array(xs) => {
			4u8.hash(state);
			xs.len().hash(state);
			for x in xs {
				hash_canonical_json(x, state);
			}
		}
		Value::Object(xs) => {
			5u8.hash(state);
			let mut keys: Vec<&String> = xs.keys().collect();
			keys.sort();
			keys.len().hash(state);
			for k in keys {
				k.hash(state);
				hash_canonical_json(&xs[k], state);
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use std::collections::hash_map::DefaultHasher;
	use std::future::Future;
	use std::hash::{Hash, Hasher};
	use std::net::SocketAddr;
	use std::pin::Pin;
	use std::time::Instant;

	use parking_lot::Mutex;
	use serde_json::json;
	use tokio_util::sync::CancellationToken;

	use super::*;
	use crate::conn_context::{ConnId, Transport};
	use crate::flow_log::{FlowLogEvent, FlowLogSink};

	struct PassPeek;
	#[async_trait]
	impl L4PeekMiddleware for PassPeek {
		async fn run(
			&self,
			_peek: &[u8],
			_conn: &Arc<ConnContext>,
			_ctx: &mut FlowCtx,
		) -> Result<Decision, Error> {
			Ok(Decision::Continue)
		}
	}

	struct PassBytes;
	#[async_trait]
	impl L4BytesMiddleware for PassBytes {
		async fn run(
			&self,
			_l4: &mut L4Conn,
			_conn: &Arc<ConnContext>,
			_ctx: &mut FlowCtx,
		) -> Result<Decision, Error> {
			Ok(Decision::Continue)
		}
	}

	struct PassReq;
	#[async_trait]
	impl L7RequestMiddleware for PassReq {
		async fn run(
			&self,
			_req: &mut Request,
			_conn: &Arc<ConnContext>,
			_ctx: &mut FlowCtx,
		) -> Result<Decision, Error> {
			Ok(Decision::Continue)
		}
	}

	struct PassResp;
	#[async_trait]
	impl L7ResponseMiddleware for PassResp {
		async fn run(
			&self,
			_resp: &mut Response,
			_conn: &Arc<ConnContext>,
			_ctx: &mut FlowCtx,
		) -> Result<Decision, Error> {
			Ok(Decision::Continue)
		}
	}

	// Compile-time assertion helper: the type `F` must be `Send`. `async_trait`
	// rewrites `async fn run(...)` to return `Pin<Box<dyn Future + Send>>`, so
	// every `run` invocation's future must satisfy this bound — that is the
	// load-bearing contract per spec/crates/engine.md § _Async Send via async_trait_.
	fn assert_send<F: Send>(_: &F) {}

	struct NullSink;
	impl FlowLogSink for NullSink {
		fn emit(&self, _event: FlowLogEvent) {}
	}

	fn make_conn_context() -> Arc<ConnContext> {
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

	// `async_trait` makes these traits dyn-compatible. `MiddlewareInst` stores
	// each variant as `Arc<dyn Trait>` per spec/crates/engine.md § _Symbolic forms_
	// and § _Async Send via async_trait_; constructing that exact shape from a
	// concrete impl is the contract we guard.

	#[test]
	fn l4_peek_is_constructible_as_arc_dyn_send_sync() {
		let m: Arc<dyn L4PeekMiddleware + Send + Sync> = Arc::new(PassPeek);
		// The trait-object Arc coerces to the bare `Arc<dyn Trait>` shape used
		// by `MiddlewareInst::L4Peek(Arc<dyn L4PeekMiddleware>)` in engine.
		let _: Arc<dyn L4PeekMiddleware> = m;
	}

	#[test]
	fn l4_bytes_is_constructible_as_arc_dyn_send_sync() {
		let m: Arc<dyn L4BytesMiddleware + Send + Sync> = Arc::new(PassBytes);
		let _: Arc<dyn L4BytesMiddleware> = m;
	}

	#[test]
	fn l7_request_is_constructible_as_arc_dyn_send_sync() {
		let m: Arc<dyn L7RequestMiddleware + Send + Sync> = Arc::new(PassReq);
		let _: Arc<dyn L7RequestMiddleware> = m;
	}

	#[test]
	fn l7_response_is_constructible_as_arc_dyn_send_sync() {
		let m: Arc<dyn L7ResponseMiddleware + Send + Sync> = Arc::new(PassResp);
		let _: Arc<dyn L7ResponseMiddleware> = m;
	}

	fn make_flow_ctx(conn_id: ConnId) -> FlowCtx {
		FlowCtx {
			span: tracing::Span::none(),
			log: Arc::new(NullSink),
			cancel: CancellationToken::new(),
			verbosity: crate::flow_log::FlowLogVerbosity::Trajectory,
			trajectory: crate::flow_log::TrajectoryBuilder::new(conn_id, crate::ir::NodeId::new(0), 0),
		}
	}

	#[test]
	fn l4_peek_run_returns_send_future() {
		let m: Arc<dyn L4PeekMiddleware> = Arc::new(PassPeek);
		let conn = make_conn_context();
		let mut ctx = make_flow_ctx(conn.id);
		let peek: &[u8] = &[];
		// Exact-type coercion into `Pin<Box<dyn Future + Send>>` — the async_trait
		// signature. Fails to compile if a future becomes `!Send`.
		let fut: Pin<Box<dyn Future<Output = Result<Decision, Error>> + Send + '_>> =
			m.run(peek, &conn, &mut ctx);
		assert_send(&fut);
		drop(fut);
	}

	#[test]
	fn l7_request_run_returns_send_future() {
		let m: Arc<dyn L7RequestMiddleware> = Arc::new(PassReq);
		let conn = make_conn_context();
		let mut ctx = make_flow_ctx(conn.id);
		let mut req: Request =
			http::Request::builder().uri("/").body(crate::body::Body::Empty).expect("build req");
		let fut: Pin<Box<dyn Future<Output = Result<Decision, Error>> + Send + '_>> =
			m.run(&mut req, &conn, &mut ctx);
		assert_send(&fut);
		drop(fut);
	}

	#[test]
	fn l7_response_run_returns_send_future() {
		let m: Arc<dyn L7ResponseMiddleware> = Arc::new(PassResp);
		let conn = make_conn_context();
		let mut ctx = make_flow_ctx(conn.id);
		let mut resp: Response =
			http::Response::builder().status(200).body(crate::body::Body::Empty).expect("build resp");
		let fut: Pin<Box<dyn Future<Output = Result<Decision, Error>> + Send + '_>> =
			m.run(&mut resp, &conn, &mut ctx);
		assert_send(&fut);
		drop(fut);
	}

	#[test]
	fn l7_request_needs_body_defaults_to_false() {
		assert!(!L7RequestMiddleware::needs_body(&PassReq));
	}

	#[test]
	fn l7_response_needs_body_defaults_to_false() {
		assert!(!L7ResponseMiddleware::needs_body(&PassResp));
	}

	#[test]
	fn middleware_kind_serde_round_trip_per_variant() {
		for k in [
			MiddlewareKind::L4Peek,
			MiddlewareKind::L4Bytes,
			MiddlewareKind::L7Request,
			MiddlewareKind::L7Response,
		] {
			let encoded = serde_json::to_string(&k).expect("serialize");
			let decoded: MiddlewareKind = serde_json::from_str(&encoded).expect("deserialize");
			assert_eq!(decoded, k);
		}
	}

	#[test]
	fn decision_and_shortcircuit_construct_per_variant() {
		let _ = Decision::Continue;
		let _ = Decision::Short(ShortCircuit::Close(CloseReason::Graceful));
		let _ = ShortCircuit::Close(CloseReason::PolicyDenied("over quota".into()));
		let _ = ShortCircuit::Close(CloseReason::ProtocolError("bad frame".into()));
	}

	#[test]
	fn close_reason_construct_per_variant() {
		let _ = CloseReason::Graceful;
		let _ = CloseReason::PolicyDenied(std::borrow::Cow::Borrowed("over quota"));
		let _ = CloseReason::ProtocolError(std::borrow::Cow::Owned(String::from("bad frame")));
		let _ = CloseReason::Cancelled;
	}

	fn hash_of<T: Hash>(v: &T) -> u64 {
		let mut h = DefaultHasher::new();
		v.hash(&mut h);
		h.finish()
	}

	fn sym_ref(args: serde_json::Value) -> SymbolicMiddlewareRef {
		SymbolicMiddlewareRef {
			name: Arc::from("rate_limit"),
			args,
			kind: MiddlewareKind::L7Request,
			stateless: true,
			needs_body: false,
			on_error: None,
		}
	}

	#[test]
	fn symbolic_ref_args_hash_is_object_key_order_insensitive() {
		// Manually build both maps with opposite insertion orders to defeat
		// serde_json::from_str's preserve-insertion-order backend.
		let mut a = serde_json::Map::new();
		a.insert("a".to_string(), json!(1));
		a.insert("b".to_string(), json!(2));
		let mut b = serde_json::Map::new();
		b.insert("b".to_string(), json!(2));
		b.insert("a".to_string(), json!(1));

		let lhs = sym_ref(serde_json::Value::Object(a));
		let rhs = sym_ref(serde_json::Value::Object(b));

		assert_eq!(lhs, rhs);
		assert_eq!(hash_of(&lhs), hash_of(&rhs));
	}

	#[test]
	fn symbolic_ref_nested_object_key_order_is_ignored() {
		let lhs = sym_ref(json!({ "outer": { "x": 1, "y": 2 } }));
		// Build the inner map with swapped order by hand.
		let mut inner = serde_json::Map::new();
		inner.insert("y".to_string(), json!(2));
		inner.insert("x".to_string(), json!(1));
		let mut outer = serde_json::Map::new();
		outer.insert("outer".to_string(), serde_json::Value::Object(inner));
		let rhs = sym_ref(serde_json::Value::Object(outer));

		assert_eq!(lhs, rhs);
		assert_eq!(hash_of(&lhs), hash_of(&rhs));
	}

	#[test]
	fn symbolic_ref_arrays_are_order_sensitive() {
		let lhs = sym_ref(json!({ "xs": [1, 2] }));
		let rhs = sym_ref(json!({ "xs": [2, 1] }));
		assert_ne!(lhs, rhs);
	}

	#[test]
	fn symbolic_ref_differs_on_name() {
		let a = sym_ref(json!({}));
		let mut b = sym_ref(json!({}));
		b.name = Arc::from("other");
		assert_ne!(a, b);
	}

	#[test]
	fn symbolic_ref_differs_on_kind() {
		let a = sym_ref(json!({}));
		let mut b = sym_ref(json!({}));
		b.kind = MiddlewareKind::L4Peek;
		assert_ne!(a, b);
	}

	#[test]
	fn symbolic_ref_differs_on_stateless() {
		let a = sym_ref(json!({}));
		let mut b = sym_ref(json!({}));
		b.stateless = false;
		assert_ne!(a, b);
	}

	#[test]
	fn symbolic_ref_differs_on_needs_body() {
		let a = sym_ref(json!({}));
		let mut b = sym_ref(json!({}));
		b.needs_body = true;
		assert_ne!(a, b);
	}

	#[test]
	fn symbolic_ref_differs_on_on_error() {
		let a = sym_ref(json!({}));
		let mut b = sym_ref(json!({}));
		b.on_error = Some(NodeId::new(3));
		assert_ne!(a, b);
	}

	#[test]
	fn symbolic_ref_same_name_but_distinct_args_are_unequal() {
		let a = sym_ref(json!({ "limit": 100 }));
		let b = sym_ref(json!({ "limit": 200 }));
		assert_ne!(a, b);
	}

	// Dry-run JSON wire-format contract: SymbolicMiddlewareRef participates
	// in the compiled-form JSON per spec/flow-model.md § _The compiled form_. The
	// whole struct uses derive(Serialize/Deserialize); all fields round-trip.
	// PartialEq uses canonical-json equality on `args`, so key-order
	// perturbation must still compare equal after a round-trip.

	#[test]
	fn symbolic_middleware_ref_round_trip_preserves_all_fields() {
		let m = SymbolicMiddlewareRef {
			name: Arc::from("rate_limit"),
			args: json!({ "rate": 100 }),
			kind: MiddlewareKind::L7Request,
			stateless: false,
			needs_body: false,
			on_error: Some(NodeId::new(5)),
		};
		let encoded = serde_json::to_string(&m).expect("serialize");
		let decoded: SymbolicMiddlewareRef = serde_json::from_str(&encoded).expect("deserialize");
		assert_eq!(decoded.name, m.name);
		assert_eq!(decoded.kind, m.kind);
		assert_eq!(decoded.stateless, m.stateless);
		assert_eq!(decoded.needs_body, m.needs_body);
		assert_eq!(decoded.on_error, m.on_error);
		assert_eq!(decoded, m);
	}

	#[test]
	fn symbolic_middleware_ref_round_trip_args_are_canonical_key_order_insensitive() {
		// Build an args value whose serialized form has a deliberate key order.
		let mut obj = serde_json::Map::new();
		obj.insert("b".to_string(), json!(1));
		obj.insert("a".to_string(), json!(2));
		let m = SymbolicMiddlewareRef {
			name: Arc::from("mw"),
			args: serde_json::Value::Object(obj),
			kind: MiddlewareKind::L7Request,
			stateless: true,
			needs_body: false,
			on_error: None,
		};
		let encoded = serde_json::to_string(&m).expect("serialize");
		let decoded: SymbolicMiddlewareRef = serde_json::from_str(&encoded).expect("deserialize");
		// PartialEq on SymbolicMiddlewareRef uses canonical-json equality on args,
		// so any post-round-trip key reshuffling remains == to the original.
		assert_eq!(decoded, m);
	}
}
