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
#[serde(rename_all = "snake_case")]
pub enum ExternalPluginDriver {
	/// HTTP/HTTPS POST to a URL.
	Http { url: String },
	/// HTTP POST over a Unix Domain Socket.
	Unix { path: String },
	/// Execute a command/program with arguments and environment variables.
	/// Inputs are sent via Stdin (JSON), output is read from Stdout (JSON).
	Command {
		/// The program to execute (e.g., "python3", "/usr/bin/node", "./my-plugin").
		program: String,
		/// Arguments to pass to the program (e.g., ["script.py", "-v"]).
		#[serde(default)]
		args: Vec<String>,
		/// Additional environment variables to set for the process.
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
	/// Use Cow to support both static strings (internal) and owned strings (external).
	pub name: Cow<'static, str>,
	pub required: bool,
	pub param_type: ParamType,
}

pub type ResolvedInputs = HashMap<String, Value>;

#[derive(Serialize, Deserialize, Debug)]
pub struct MiddlewareOutput {
	pub branch: Cow<'static, str>,
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
	/// Returns the list of possible output branches.
	/// Uses Cow to support both static (internal) and dynamic (future external) branch names.
	fn output(&self) -> Vec<Cow<'static, str>>;
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
