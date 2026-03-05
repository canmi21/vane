/* src/api/src/schemas/applications.rs */

use serde::Serialize;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct ApplicationSummary {
	pub protocol: String,
	pub source_format: Option<String>,
	pub active: bool,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct ApplicationListData {
	pub applications: Vec<ApplicationSummary>,
	pub supported_protocols: Vec<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct ApplicationDetail {
	pub protocol: String,
	pub source_format: String,
	pub pipeline: vane_engine::engine::interfaces::ProcessingStep,
}

// --- Response Schemas ---

#[derive(Serialize, ToSchema)]
pub struct ApplicationListResponse {
	pub status: String,
	pub data: ApplicationListData,
}

#[derive(Serialize, ToSchema)]
pub struct ApplicationDetailResponse {
	pub status: String,
	pub data: ApplicationDetail,
}
