/* src/api/src/schemas/system.rs */

use serde::Serialize;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct PackageInfo {
	pub name: String,
	pub version: String,
	pub author: String,
	pub license: String,
	pub repository: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct BuildInfo {
	pub rust_version: String,
	pub cargo_version: String,
	pub build_date: String,
	pub git_commit: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct RuntimeInfo {
	pub arch: String,
	pub platform: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct SystemInfo {
	pub package: PackageInfo,
	pub build: BuildInfo,
	pub runtime: RuntimeInfo,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct HealthStatus {
	pub healthy: bool,
	pub uptime_secs: u64,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct SystemStatusDetails {
	#[schema(value_type = Object)]
	pub listeners: serde_json::Value,
	#[schema(value_type = Object)]
	pub plugins: serde_json::Value,
	#[schema(value_type = Object)]
	pub resources: serde_json::Value,
}

// --- Explicit Response Schemas for OpenAPI ---

#[derive(Serialize, ToSchema)]
pub struct SystemInfoResponse {
	pub status: String,
	pub data: SystemInfo,
}

#[derive(Serialize, ToSchema)]
pub struct HealthStatusResponse {
	pub status: String,
	pub data: HealthStatus,
}

#[derive(Serialize, ToSchema)]
pub struct SystemStatusResponse {
	pub status: String,
	pub data: SystemStatusDetails,
}
