/* src/modules/plugins/model.rs */

use crate::modules::kv::KvStore;
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{any::Any, collections::HashMap, net::SocketAddr, sync::Arc};
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
#[serde(rename_all = "snake_case")]
pub enum ExternalPluginDriver {
	/// HTTP/HTTPS POST to a URL.
	Http { url: String },
	/// HTTP POST over a Unix Domain Socket.
	Unix { path: String },
	/// Execute a local binary with env vars, capture stdout.
	Bin {
		path: String,
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
	/// If defined, these params are required when invoking the plugin.
	#[serde(default)]
	pub params: Vec<ExternalParamDef>,
}

/// Simplified param definition for serialization
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ExternalParamDef {
	pub name: String,
	pub required: bool,
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
	pub name: &'static str,
	pub required: bool,
	pub param_type: ParamType,
}

pub type ResolvedInputs = HashMap<String, Value>;

pub struct MiddlewareOutput {
	pub branch: &'static str,
	pub write_to_kv: Option<HashMap<String, String>>,
}

pub enum ConnectionObject {
	Tcp(TcpStream),
	Udp {
		socket: Arc<UdpSocket>,
		datagram: Vec<u8>,
		client_addr: SocketAddr,
	},
}

/// A generic base trait for all plugins.
/// It now includes helper methods for trait object casting.
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
	fn output(&self) -> Vec<&'static str>;
	async fn execute(&self, inputs: ResolvedInputs) -> Result<MiddlewareOutput>;
}

/// A trait for "Terminator" plugins, made object-safe with async-trait.
#[async_trait]
pub trait Terminator: Plugin {
	async fn execute(
		&self,
		inputs: ResolvedInputs,
		kv: &KvStore,
		conn: ConnectionObject,
	) -> Result<()>;
}
