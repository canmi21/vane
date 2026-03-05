/* src/api/schemas/plugins.rs */

use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct PluginSummary {
	pub name: String,
	pub role: String,
	#[serde(rename = "type")]
	pub type_name: String,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub healthy: Option<bool>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct PluginList {
	pub internal: Vec<PluginSummary>,
	pub external: Vec<PluginSummary>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct ParamDefResponse {
	pub name: String,
	pub required: bool,
	pub param_type: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct PluginDetail {
	pub name: String,
	#[serde(rename = "type")]
	pub type_name: String,
	pub role: String,
	pub params: Vec<ParamDefResponse>,
	pub supported_protocols: Vec<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub driver: Option<serde_json::Value>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub healthy: Option<bool>,
}

#[derive(Deserialize, ToSchema, IntoParams)]
pub struct ListPluginsQuery {
	#[serde(rename = "type")]
	pub type_name: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct PluginOperationResult {
	pub status: String,
	pub name: String,
}

// --- Explicit Response Schemas for OpenAPI ---

#[derive(Serialize, ToSchema)]
pub struct PluginListResponse {
	pub status: String,
	pub data: PluginList,
}

#[derive(Serialize, ToSchema)]
pub struct PluginDetailResponse {
	pub status: String,
	pub data: PluginDetail,
}

#[derive(Serialize, ToSchema)]
pub struct PluginOperationResponse {
	pub status: String,
	pub data: PluginOperationResult,
}
