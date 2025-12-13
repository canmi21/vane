/* src/modules/plugins/model.rs */

use crate::modules::kv::KvStore;
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{any::Any, borrow::Cow, collections::HashMap, net::SocketAddr, sync::Arc};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpStream, UdpSocket};

// --- Configuration Data Structures ---

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct PluginInstance {
	#[serde(default)]
	pub input: HashMap<String, Value>,
	#[serde(default)]
	pub output: HashMap<String, ProcessingStep>,
}

pub type ProcessingStep = HashMap<String, PluginInstance>;

// --- External Plugin Models ---

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum ExternalPluginDriver {
	/// HTTP/HTTPS POST to a URL.
	Http { url: String },
	/// HTTP POST over a Unix Domain Socket.
	Unix { path: String },
	/// Execute a command/program with arguments and environment variables.
	Command {
		program: String,
		#[serde(default)]
		args: Vec<String>,
		#[serde(default)]
		env: HashMap<String, String>,
	},
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PluginRole {
	Middleware,
	Terminator,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ExternalPluginConfig {
	pub name: String,
	pub role: PluginRole,
	pub driver: ExternalPluginDriver,
	#[serde(default)]
	pub params: Vec<ExternalParamDef>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ExternalParamDef {
	pub name: String,
	pub required: bool,
}

// --- API Contract (Mirroring core/response.rs) ---

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

/// A trait alias for any stream that supports async reading and writing.
pub trait ByteStream: AsyncRead + AsyncWrite + Unpin + Send + Sync {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + Sync> ByteStream for T {}

/// The runtime object passed through the flow engine.
/// It evolves as it moves up the layers (L4 -> L4+ -> L7).
#[derive(Debug)]
pub enum ConnectionObject {
	/// Layer 4: Raw TCP Stream
	Tcp(TcpStream),

	/// Layer 4: Raw UDP Socket Context
	Udp {
		socket: Arc<UdpSocket>,
		datagram: Vec<u8>,
		client_addr: SocketAddr,
	},

	/// Layer 4+ / Layer 5: Encrypted or Abstracted Stream
	Stream(Box<dyn ByteStream>),
}

// Manual Debug implementation for Box<dyn ByteStream> because traits don't auto-derive Debug.
impl std::fmt::Debug for Box<dyn ByteStream> {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "ByteStream(...)")
	}
}

/// Defines the operational layers within Vane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
	L4,
	L4Plus,
	L7,
}

/// The result of a Terminator execution.
#[derive(Debug)]
pub enum TerminatorResult {
	/// The connection flow has been completed (proxied, aborted, or handled).
	Finished,

	/// The connection should be upgraded to a higher protocol layer.
	/// The Terminator must return the `ConnectionObject` (ownership transfer)
	/// so the engine can pass it to the next layer.
	Upgrade {
		protocol: String,
		conn: ConnectionObject,
	},
}

/// A generic base trait for all plugins.
pub trait Plugin: Send + Sync + Any {
	fn name(&self) -> &str;
	fn params(&self) -> Vec<ParamDef>;
	fn as_any(&self) -> &dyn Any;

	fn as_middleware(&self) -> Option<&dyn Middleware> {
		None
	}

	fn as_terminator(&self) -> Option<&dyn Terminator> {
		None
	}
}

#[async_trait]
pub trait Middleware: Plugin {
	fn output(&self) -> Vec<Cow<'static, str>>;
	async fn execute(&self, inputs: ResolvedInputs) -> Result<MiddlewareOutput>;
}

#[async_trait]
pub trait Terminator: Plugin {
	fn supported_layers(&self) -> Vec<Layer>;
	async fn execute(
		&self,
		inputs: ResolvedInputs,
		kv: &KvStore,
		conn: ConnectionObject,
	) -> Result<TerminatorResult>;
}
