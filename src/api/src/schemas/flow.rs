/* src/api/src/schemas/flow.rs */

use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use vane_engine::engine::interfaces::ProcessingStep;

#[derive(Serialize, Deserialize, ToSchema)]
pub struct FlowConfig {
	pub connection: ProcessingStep,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct FlowConfigData {
	pub source_format: String,
	pub content: FlowConfig,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct FlowConfigWritten {
	pub port: u16,
	pub protocol: String,
	pub written_to: String,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub converted_from: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct ValidationResult {
	pub valid: bool,
	pub plugins_used: Vec<String>,
	pub warnings: Vec<String>,
}

#[derive(Deserialize, ToSchema, IntoParams)]
pub struct ValidateQuery {
	pub validate_only: Option<bool>,
}

// --- Explicit Response Schemas for OpenAPI ---

#[derive(Serialize, ToSchema)]
pub struct FlowConfigResponse {
	pub status: String,
	pub data: FlowConfigData,
}

#[derive(Serialize, ToSchema)]
pub struct FlowConfigWrittenResponse {
	pub status: String,
	pub data: FlowConfigWritten,
}

#[derive(Serialize, ToSchema)]
pub struct ValidationResultResponse {
	pub status: String,
	pub data: ValidationResult,
}
