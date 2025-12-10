/* src/modules/plugins/model.rs */

use crate::modules::kv::KvStore;
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{any::Any, borrow::Cow, collections::HashMap, net::SocketAddr, sync::Arc};
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

/// Represents the strict JSON response format expected from external plugins.
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

pub enum ConnectionObject {
	Tcp(TcpStream),
	Udp {
		socket: Arc<UdpSocket>,
		datagram: Vec<u8>,
		client_addr: SocketAddr,
	},
}

/// Defines the operational layers within Vane.
/// Terminators must declare which layers they support to ensure architectural safety.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
	/// Layer 4: Raw TCP/UDP Transport (Listener Level)
	L4,
	/// Layer 4+: Encrypted/Enhanced Transport (e.g., TLS, QUIC) (Resolver Level)
	L4Plus,
	/// Layer 7: Application Layer (e.g., HTTP)
	L7,
}

/// The result of a Terminator execution.
#[derive(Debug)]
pub enum TerminatorResult {
	/// The connection flow has been completed (proxied, aborted, or handled).
	/// The engine should stop processing this connection.
	Finished,

	/// The connection should be upgraded to a higher protocol layer.
	/// The engine should keep the connection alive and transfer control to the
	/// specified protocol resolver (e.g., "tls", "http").
	Upgrade { protocol: String },
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

/// A trait for "Middleware" plugins, made object-safe with async-trait.
#[async_trait]
pub trait Middleware: Plugin {
	fn output(&self) -> Vec<Cow<'static, str>>;
	async fn execute(&self, inputs: ResolvedInputs) -> Result<MiddlewareOutput>;
}

/// A trait for "Terminator" plugins, made object-safe with async-trait.
#[async_trait]
pub trait Terminator: Plugin {
	/// Returns the layers where this terminator is valid.
	fn supported_layers(&self) -> Vec<Layer>;

	/// Executes the termination logic.
	/// Returns a `TerminatorResult` indicating whether to finish or upgrade the flow.
	async fn execute(
		&self,
		inputs: ResolvedInputs,
		kv: &KvStore,
		conn: ConnectionObject,
	) -> Result<TerminatorResult>;
}
