/* src/api/src/schemas/config.rs */

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Deserialize, ToSchema)]
pub struct ReloadRequest {
	/// Optional list of components to reload (e.g., ["ports", "nodes", "certs"]).
	/// If empty, reloads all.
	pub components: Option<Vec<String>>,
}

#[derive(Serialize, ToSchema)]
pub struct ReloadResult {
	pub reloaded: Vec<String>,
	pub timestamp: String,
}

#[derive(Serialize, ToSchema)]
pub struct ImportResult {
	pub imported: std::collections::HashMap<String, usize>,
}

// --- Response Schemas ---

#[derive(Serialize, ToSchema)]
pub struct ReloadResponse {
	pub status: String,
	pub data: ReloadResult,
}

#[derive(Serialize, ToSchema)]
pub struct ImportResponse {
	pub status: String,
	pub data: ImportResult,
}
