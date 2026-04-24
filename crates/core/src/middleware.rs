use std::hash::Hash;
use std::sync::Arc;

use crate::body::{Request, Response};
use crate::conn_context::ConnContext;
use crate::error::Error;
use crate::flow_ctx::FlowCtx;
use crate::ir::NodeId;
use crate::l4::L4Conn;

#[trait_variant::make(L4PeekMiddleware: Send)]
pub trait L4PeekMiddlewareLocal {
	async fn run(
		&self,
		peek: &[u8],
		conn: &Arc<ConnContext>,
		ctx: &mut FlowCtx<'_>,
	) -> Result<Decision, Error>;
}

#[trait_variant::make(L4BytesMiddleware: Send)]
pub trait L4BytesMiddlewareLocal {
	async fn run(
		&self,
		l4: &mut L4Conn,
		conn: &Arc<ConnContext>,
		ctx: &mut FlowCtx<'_>,
	) -> Result<Decision, Error>;
}

#[trait_variant::make(L7RequestMiddleware: Send)]
pub trait L7RequestMiddlewareLocal {
	async fn run(
		&self,
		req: &mut Request,
		conn: &Arc<ConnContext>,
		ctx: &mut FlowCtx<'_>,
	) -> Result<Decision, Error>;

	fn needs_body(&self) -> bool {
		false
	}
}

#[trait_variant::make(L7ResponseMiddleware: Send)]
pub trait L7ResponseMiddlewareLocal {
	async fn run(
		&self,
		resp: &mut Response,
		conn: &Arc<ConnContext>,
		ctx: &mut FlowCtx<'_>,
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
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub enum MiddlewareKind {
	L4Peek,
	L4Bytes,
	L7Request,
	L7Response,
}

#[derive(Clone, Debug)]
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
