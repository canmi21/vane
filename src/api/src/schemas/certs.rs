/* src/api/src/schemas/certs.rs */

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct CertSummary {
	pub id: String,
	pub subject: String,
	pub issuer: String,
	pub not_before: String,
	pub not_after: String,
	pub fingerprint_sha256: String,
	pub auto_generated: bool,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct CertDetail {
	pub id: String,
	pub subject: String,
	pub issuer: String,
	pub not_before: String,
	pub not_after: String,
	pub expires_in_days: i64,
	pub fingerprint_sha256: String,
	pub san: Vec<String>,
	pub key_type: String,
}

#[derive(Deserialize, ToSchema)]
pub struct CertUploadRequest {
	pub cert_pem: String,
	pub key_pem: String,
}

#[derive(Serialize, ToSchema)]
pub struct CertOperationResult {
	pub id: String,
	pub created: bool,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub subject: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub not_after: Option<String>,
}

// --- Response Schemas ---

#[derive(Serialize, ToSchema)]
pub struct CertListResponse {
	pub status: String,
	pub data: Vec<CertSummary>,
}

#[derive(Serialize, ToSchema)]
pub struct CertDetailResponse {
	pub status: String,
	pub data: CertDetail,
}

#[derive(Serialize, ToSchema)]
pub struct CertOperationResponse {
	pub status: String,
	pub data: CertOperationResult,
}
