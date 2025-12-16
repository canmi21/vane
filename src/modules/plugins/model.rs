/* src/modules/plugins/model.rs */

use crate::modules::kv::KvStore;
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{any::Any, borrow::Cow, collections::HashMap, fmt, net::SocketAddr, sync::Arc};
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
		datagram: Vec<u8>,
		client_addr: SocketAddr,
	},
	Stream(Box<dyn ByteStream>),
	// Virtual connection for L7 internal flows or abstract contexts
	Virtual(String),
}

impl fmt::Debug for ConnectionObject {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			ConnectionObject::Tcp(stream) => f
				.debug_struct("ConnectionObject::Tcp")
				.field("peer_addr", &stream.peer_addr().ok())
				.finish(),
			ConnectionObject::Udp { client_addr, .. } => f
				.debug_struct("ConnectionObject::Udp")
				.field("client_addr", client_addr)
				.finish(),
			ConnectionObject::Stream(_) => f
				.debug_struct("ConnectionObject::Stream")
				.field("type", &"Box<dyn ByteStream>")
				.finish(),
			ConnectionObject::Virtual(desc) => f
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
		parent_path: String, // Added field to track path continuity
	},
}

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
		kv: &mut KvStore, // Fixed signature
		conn: ConnectionObject,
	) -> Result<TerminatorResult>;
}
