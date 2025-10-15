/* engine/src/modules/certs/manager.rs */

use crate::{common::response, daemon::config, modules::certs::analysis};
use axum::{
	Json,
	extract::Path,
	http::StatusCode,
	response::{IntoResponse, Response},
};
use fancy_log::{LogLevel, log};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// --- Helper Functions ---

/// Converts a domain name to a filesystem-safe base name (e.g., "*.example.com" -> "_.example.com").
fn domain_to_file_base(domain: &str) -> String {
	domain.replace('*', "_")
}

/// Converts a filesystem base name back to a domain name.
fn file_base_to_domain(base: &str) -> String {
	base.replace('_', "*")
}

/// Finds the certificate and key files for a given domain.
async fn find_cert_files(domain: &str) -> Option<(PathBuf, PathBuf)> {
	let certs_dir = config::get_certs_dir();
	let base_name = domain_to_file_base(domain);

	let key_path = certs_dir.join(format!("{}.key", base_name));
	if !key_path.exists() {
		return None;
	}

	for ext in ["pem", "crt", "cer", "der"] {
		let cert_path = certs_dir.join(format!("{}.{}", base_name, ext));
		if cert_path.exists() {
			return Some((cert_path, key_path));
		}
	}

	None
}

// --- API Payloads ---

#[derive(Serialize)]
pub struct ListCertsResponse {
	pub certificates: Vec<String>,
}

#[derive(Deserialize)]
pub struct UploadCertPayload {
	pub cert_pem_b64: String,
	pub key_pem_b64: String,
}

// --- Axum Handlers ---

/// Lists all available certificates by scanning the certs directory.
pub async fn list_certs() -> Response {
	log(LogLevel::Debug, "GET /v1/certs called");
	let certs_dir = config::get_certs_dir();
	let mut certs = std::collections::HashSet::new(); // Use HashSet to avoid duplicates

	if !certs_dir.exists() {
		return response::success(ListCertsResponse {
			certificates: vec![],
		})
		.into_response();
	}

	let mut entries = match tokio::fs::read_dir(certs_dir).await {
		Ok(e) => e,
		Err(_) => {
			return response::error(
				StatusCode::INTERNAL_SERVER_ERROR,
				"Failed to read certs directory".to_string(),
			)
			.into_response();
		}
	};

	while let Ok(Some(entry)) = entries.next_entry().await {
		if let Some(path) = entry.path().file_stem() {
			if let Some(name) = path.to_str() {
				certs.insert(file_base_to_domain(name));
			}
		}
	}

	let mut sorted_certs: Vec<String> = certs.into_iter().collect();
	sorted_certs.sort();

	response::success(ListCertsResponse {
		certificates: sorted_certs,
	})
	.into_response()
}

/// Retrieves detailed information about a specific certificate.
pub async fn get_cert_details(Path(domain): Path<String>) -> Response {
	log(LogLevel::Debug, &format!("GET /v1/certs/{} called", domain));

	let (cert_path, _) = match find_cert_files(&domain).await {
		Some(paths) => paths,
		None => {
			return response::error(StatusCode::NOT_FOUND, "Certificate not found.".to_string())
				.into_response();
		}
	};

	let cert_bytes = match tokio::fs::read(&cert_path).await {
		Ok(bytes) => bytes,
		Err(_) => {
			return response::error(
				StatusCode::INTERNAL_SERVER_ERROR,
				"Failed to read certificate file.".to_string(),
			)
			.into_response();
		}
	};

	match analysis::parse_cert_details(&cert_bytes) {
		Ok(info) => response::success(info).into_response(),
		Err(e) => response::error(
			StatusCode::INTERNAL_SERVER_ERROR,
			format!("Failed to parse certificate: {}", e),
		)
		.into_response(),
	}
}

/// Uploads and saves a new certificate and its private key.
pub async fn upload_cert(
	Path(domain): Path<String>,
	Json(payload): Json<UploadCertPayload>,
) -> Response {
	log(LogLevel::Info, &format!("POST /v1/certs/{} called", domain));

	let cert_bytes = match base64::Engine::decode(
		&base64::engine::general_purpose::STANDARD,
		payload.cert_pem_b64,
	) {
		Ok(b) => b,
		Err(_) => {
			return response::error(
				StatusCode::BAD_REQUEST,
				"Invalid base64 for certificate.".to_string(),
			)
			.into_response();
		}
	};

	let key_bytes = match base64::Engine::decode(
		&base64::engine::general_purpose::STANDARD,
		payload.key_pem_b64,
	) {
		Ok(b) => b,
		Err(_) => {
			return response::error(
				StatusCode::BAD_REQUEST,
				"Invalid base64 for private key.".to_string(),
			)
			.into_response();
		}
	};

	// Validate that the certificate is parseable before saving.
	if let Err(e) = analysis::parse_cert_details(&cert_bytes) {
		return response::error(
			StatusCode::BAD_REQUEST,
			format!("Provided certificate is invalid: {}", e),
		)
		.into_response();
	}

	let certs_dir = config::get_certs_dir();
	let base_name = domain_to_file_base(&domain);
	let cert_path = certs_dir.join(format!("{}.pem", base_name));
	let key_path = certs_dir.join(format!("{}.key", base_name));

	if tokio::fs::write(&cert_path, &cert_bytes).await.is_err()
		|| tokio::fs::write(&key_path, &key_bytes).await.is_err()
	{
		return response::error(
			StatusCode::INTERNAL_SERVER_ERROR,
			"Failed to write certificate or key file.".to_string(),
		)
		.into_response();
	}

	log(
		LogLevel::Info,
		&format!("Certificate for {} successfully saved.", domain),
	);
	(StatusCode::CREATED, "Certificate and key saved.").into_response()
}

/// Deletes a certificate and its corresponding key file.
pub async fn delete_cert(Path(domain): Path<String>) -> Response {
	log(
		LogLevel::Info,
		&format!("DELETE /v1/certs/{} called", domain),
	);

	let (cert_path, key_path) = match find_cert_files(&domain).await {
		Some(paths) => paths,
		None => {
			return response::error(StatusCode::NOT_FOUND, "Certificate not found.".to_string())
				.into_response();
		}
	};

	let cert_del = tokio::fs::remove_file(&cert_path).await;
	let key_del = tokio::fs::remove_file(&key_path).await;

	if cert_del.is_err() || key_del.is_err() {
		log(
			LogLevel::Error,
			&format!("Failed to delete one or both files for domain {}", domain),
		);
		return response::error(
			StatusCode::INTERNAL_SERVER_ERROR,
			"Failed to completely delete certificate and key.".to_string(),
		)
		.into_response();
	}

	log(
		LogLevel::Info,
		&format!("Certificate for {} successfully deleted.", domain),
	);
	StatusCode::NO_CONTENT.into_response()
}
