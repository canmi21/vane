use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionInfo {
	pub id: String,
	pub peer_addr: String,
	pub server_addr: String,
	pub listen_port: u16,
	pub phase: Phase,
	pub forward_target: Option<String>,
	pub started_at_unix_ms: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
pub enum Phase {
	Accepted,
	Forwarding,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ListConnectionsOutput {
	pub total: u32,
	pub connections: Vec<ConnectionInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SystemInfoOutput {
	pub version: String,
	pub started_at_unix_ms: String,
	pub active_listeners: u32,
	pub total_connections: u32,
	pub configured_rules: u32,
}

/// Opaque JSON value — exported as `unknown` in TypeScript.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct JsonBlob(pub serde_json::Value);

impl specta::Type for JsonBlob {
	fn definition(types: &mut specta::Types) -> specta::datatype::DataType {
		String::definition(types)
	}
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct GetConfigOutput {
	pub config: JsonBlob,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct UpdateConfigInput {
	pub config: JsonBlob,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UpdateConfigOutput {
	pub ok: bool,
	pub validation_errors: Vec<ValidationIssue>,
	pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ValidationIssue {
	pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct CompileListenersInput {
	pub listeners: Vec<vane_engine::config::ListenerRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct CompileListenersOutput {
	pub ok: bool,
	pub listeners: Vec<vane_engine::config::CompiledListener>,
	pub error: Option<String>,
}
