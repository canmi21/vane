/* src/engine/interfaces.rs */

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{any::Any, borrow::Cow, collections::HashMap, fmt, net::SocketAddr, sync::Arc};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpStream, UdpSocket};
use vane_primitives::kv::KvStore;

#[cfg(feature = "console")]
use utoipa::ToSchema;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "console", derive(ToSchema))]
pub struct PluginInstance {
	#[serde(default)]
	#[cfg_attr(feature = "console", schema(value_type = Object))]
	pub input: HashMap<String, Value>,
	#[serde(default)]
	#[cfg_attr(feature = "console", schema(value_type = Object))]
	pub output: HashMap<String, ProcessingStep>,
}

pub type ProcessingStep = HashMap<String, PluginInstance>;

// --- External Plugin Models ---
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "type")]
#[cfg_attr(feature = "console", derive(ToSchema))]
pub enum ExternalPluginDriver {
	Http {
		url: String,
	},
	Unix {
		path: String,
	},
	Command {
		program: String,
		#[serde(default)]
		args: Vec<String>,
		#[serde(default)]
		#[cfg_attr(feature = "console", schema(value_type = Object))]
		env: HashMap<String, String>,
	},
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "console", derive(ToSchema))]
pub enum PluginRole {
	Middleware,
	Terminator,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "console", derive(ToSchema))]
pub struct ExternalPluginConfig {
	pub name: String,
	pub role: PluginRole,
	pub driver: ExternalPluginDriver,
	#[serde(default)]
	pub params: Vec<ExternalParamDef>,
	#[serde(default)]
	pub output: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "console", derive(ToSchema))]
pub struct ExternalParamDef {
	pub name: String,
	pub required: bool,
}

// --- API Contract ---

#[derive(Deserialize, Debug)]
pub struct ExternalApiResponse<T> {
	pub status: String,
	pub data: Option<T>,
	pub message: Option<String>,
}

// --- Plugin Trait Definitions ---

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamType {
	String,
	Integer,
	Boolean,
	Bytes,
	Map,   // JSON Object
	Array, // JSON Array
	Any,   // Polymorphic (String | Map)
}

pub struct ParamDef {
	pub name: Cow<'static, str>,
	pub required: bool,
	pub param_type: ParamType,
}

pub type ResolvedInputs = HashMap<String, Value>;

#[derive(Serialize, Deserialize, Debug)]
pub struct MiddlewareOutput {
	pub branch: Cow<'static, str>,
	pub store: Option<HashMap<String, String>>,
}

// --- Connection Object Abstraction ---
pub trait ByteStream: AsyncRead + AsyncWrite + Unpin + Send + Sync {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + Sync> ByteStream for T {}

pub enum ConnectionObject {
	Tcp(TcpStream),
	Udp {
		socket: Arc<UdpSocket>,
		datagram: bytes::Bytes,
		client_addr: SocketAddr,
	},
	Stream(Box<dyn ByteStream>),
	Virtual(String),
}

impl fmt::Debug for ConnectionObject {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::Tcp(stream) => f
				.debug_struct("ConnectionObject::Tcp")
				.field("peer_addr", &stream.peer_addr().ok())
				.finish(),
			Self::Udp { client_addr, .. } => f
				.debug_struct("ConnectionObject::Udp")
				.field("client_addr", client_addr)
				.finish(),
			Self::Stream(_) => f
				.debug_struct("ConnectionObject::Stream")
				.field("type", &"Box<dyn ByteStream>")
				.finish(),
			Self::Virtual(desc) => f
				.debug_struct("ConnectionObject::Virtual")
				.field("desc", desc)
				.finish(),
		}
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
	L4,
	L4Plus,
	L7,
}

#[derive(Debug)]
pub enum TerminatorResult {
	Finished,
	Upgrade {
		protocol: String,
		conn: ConnectionObject,
		parent_path: String,
	},
}

pub trait Plugin: Send + Sync + Any {
	fn name(&self) -> &str;
	fn params(&self) -> Vec<ParamDef>;
	/// Returns the supported protocols for this plugin.
	/// Generic plugins should return an empty list or `vec!["any"]`.
	/// Protocol-specific plugins should return explicit protocols e.g., `vec!["http", "https"]`.
	fn supported_protocols(&self) -> Vec<Cow<'static, str>> {
		vec![]
	}
	fn as_any(&self) -> &dyn Any;

	fn as_middleware(&self) -> Option<&dyn Middleware> {
		None
	}

	fn as_generic_middleware(&self) -> Option<&dyn GenericMiddleware> {
		None
	}

	fn as_http_middleware(&self) -> Option<&dyn HttpMiddleware> {
		None
	}

	fn as_terminator(&self) -> Option<&dyn Terminator> {
		None
	}

	fn as_l7_middleware(&self) -> Option<&dyn L7Middleware> {
		None
	}

	fn as_l7_terminator(&self) -> Option<&dyn L7Terminator> {
		None
	}
}

/// Legacy Middleware trait (deprecated, transitioning to GenericMiddleware).
#[async_trait]
pub trait Middleware: Plugin {
	fn output(&self) -> Vec<Cow<'static, str>>;
	async fn execute(&self, inputs: ResolvedInputs) -> Result<MiddlewareOutput>;
}

/// Generic Middleware trait for cross-layer plugins (L4, L4+, L7).
///
/// Features:
/// - Restricted Input: Only receives `ResolvedInputs` (via templates).
/// - Restricted Output: Returns `MiddlewareOutput` (branch + KV updates).
/// - No Context Access: Cannot access Container or Socket directly.
/// - Execution: Flow Engine handles writing KV updates based on `flow_path`.
/// - Can be External: Supports external drivers (HTTP/Unix/Cmd).
#[async_trait]
pub trait GenericMiddleware: Plugin {
	fn output(&self) -> Vec<Cow<'static, str>>;
	async fn execute(&self, inputs: ResolvedInputs) -> Result<MiddlewareOutput>;
}

/// HTTP Protocol-Specific Middleware trait.
///
/// Features:
/// - Full Access: Receives `&mut Container` (via `Any` downcast) + `ResolvedInputs`.
/// - Stream Capable: Can manipulate Body streams, Upgrades, Headers.
/// - Internal Only: Must be implemented in Rust.
/// - Protocol Bound: Only valid in flows with HTTP context.
#[async_trait]
pub trait HttpMiddleware: Plugin {
	fn output(&self) -> Vec<Cow<'static, str>>;
	/// Context is expected to be `&mut Container`
	async fn execute(
		&self,
		context: &mut (dyn Any + Send),
		inputs: ResolvedInputs,
	) -> Result<MiddlewareOutput>;
}

#[async_trait]
pub trait L7Middleware: Plugin {
	fn output(&self) -> Vec<Cow<'static, str>>;
	async fn execute_l7(
		&self,
		context: &mut (dyn Any + Send),
		inputs: ResolvedInputs,
	) -> Result<MiddlewareOutput>;
}

#[async_trait]
pub trait Terminator: Plugin {
	fn supported_layers(&self) -> Vec<Layer>;
	async fn execute(
		&self,
		inputs: ResolvedInputs,
		kv: &mut KvStore,
		conn: ConnectionObject,
	) -> Result<TerminatorResult>;
}

/// A privileged terminator trait that grants access to the full L7 Context.
/// Used for plugins that need to signal responses (SendResponse) or inspect Body during termination.
#[async_trait]
pub trait L7Terminator: Plugin {
	async fn execute_l7(
		&self,
		context: &mut (dyn Any + Send),
		inputs: ResolvedInputs,
	) -> Result<TerminatorResult>;
}
