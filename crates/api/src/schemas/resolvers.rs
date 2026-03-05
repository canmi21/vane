/* src/api/schemas/resolvers.rs */

use serde::Serialize;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct ResolverSummary {
	pub protocol: String,
	pub source_format: Option<String>,
	pub active: bool,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct ResolverListData {
	pub resolvers: Vec<ResolverSummary>,
	pub supported_protocols: Vec<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct ResolverDetail {
	pub protocol: String,
	pub source_format: String,
	pub connection: vane_engine::engine::interfaces::ProcessingStep,
}

// --- Response Schemas ---

#[derive(Serialize, ToSchema)]
pub struct ResolverListResponse {
	pub status: String,
	pub data: ResolverListData,
}

#[derive(Serialize, ToSchema)]
pub struct ResolverDetailResponse {
	pub status: String,
	pub data: ResolverDetail,
}
