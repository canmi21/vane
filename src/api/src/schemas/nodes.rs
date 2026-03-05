/* src/api/src/schemas/nodes.rs */

use serde::Serialize;
use utoipa::ToSchema;
use vane_primitives::service_discovery::model::Node;

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct NodeListData {
	pub source_format: String,
	pub nodes: Vec<Node>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct NodeOperationResult {
	pub name: String,
}

// --- Response Schemas ---

#[derive(Serialize, ToSchema)]
pub struct NodeListResponse {
	pub status: String,
	pub data: NodeListData,
}

#[derive(Serialize, ToSchema)]
pub struct NodeDetailResponse {
	pub status: String,
	pub data: Node,
}

#[derive(Serialize, ToSchema)]
pub struct NodeOperationResponse {
	pub status: String,
	pub data: NodeOperationResult,
}
