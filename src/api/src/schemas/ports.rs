/* src/api/src/schemas/ports.rs */

use serde::Serialize;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct PortInfo {
	pub port: u16,
	pub protocols: Vec<String>,
	pub active: bool,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct ProtocolStatus {
	pub active: bool,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub source_format: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct PortDetail {
	pub port: u16,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub tcp: Option<ProtocolStatus>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub udp: Option<ProtocolStatus>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct PortCreated {
	pub port: u16,
	pub created: bool,
}

// --- Response Schemas ---

#[derive(Serialize, ToSchema)]
pub struct PortListResponse {
	pub status: String,
	pub data: Vec<PortInfo>,
}

#[derive(Serialize, ToSchema)]
pub struct PortDetailResponse {
	pub status: String,
	pub data: PortDetail,
}

#[derive(Serialize, ToSchema)]
pub struct PortCreatedResponse {
	pub status: String,
	pub data: PortCreated,
}
