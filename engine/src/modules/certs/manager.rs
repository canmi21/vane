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
use std::{collections::BTreeMap, path::PathBuf}; // Use BTreeMap for sorted keys

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

// --- API Payloads (Updated for new list_certs response) ---

#[derive(Serialize)]
pub struct CertSummary {
	pub filename: String,
	pub format: String,
	pub expires_at: String,
	pub issued_to: Vec<String>,
}

#[derive(Serialize)]
pub struct ListCertsResponse {
	// Use BTreeMap to have domains sorted alphabetically in the JSON response.
	pub certificates: BTreeMap<String, CertSummary>,
}

#[derive(Deserialize)]
pub struct UploadCertPayload {
	pub cert_pem_b64: String,
	pub key_pem_b64: String,
}

// --- Axum Handlers ---

/// Lists all available certificates with summary details.
pub async fn list_certs() -> Response {
	log(LogLevel::Debug, "GET /v1/certs called");
	let certs_dir = config::get_certs_dir();
	let mut cert_map = BTreeMap::new();

	if !certs_dir.exists() {
		return response::success(ListCertsResponse {
			certificates: cert_map,
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
		let path = entry.path();
		let filename = match path.file_name().and_then(|s| s.to_str()) {
			Some(name) => name,
			None => continue,
		};

		// --- BUG FIX: Check against a list of valid extensions ---
		let valid_extensions = [".pem", ".crt", ".cer", ".der"];
		let is_cert_file = valid_extensions.iter().any(|ext| filename.ends_with(ext));

		if filename == ".DS_Store" || !is_cert_file {
			continue;
		}
		// ---------------------------------------------------------

		let file_stem = match path.file_stem().and_then(|s| s.to_str()) {
			Some(stem) => stem,
			None => continue,
		};

		let domain = file_base_to_domain(file_stem);

		// If we already processed a cert for this domain (e.g., found .pem and .crt), skip.
		if cert_map.contains_key(&domain) {
			continue;
		}

		// Read and parse the cert for summary info
		if let Ok(bytes) = tokio::fs::read(&path).await {
			if let Ok(details) = analysis::parse_cert_details(&bytes) {
				let summary = CertSummary {
					filename: filename.to_string(),
					format: path
						.extension()
						.and_then(|s| s.to_str())
						.unwrap_or("unknown")
						.to_uppercase(),
					expires_at: details.validity.not_after,
					issued_to: details.subject_alternative_names,
				};
				cert_map.insert(domain, summary);
			}
		}
	}

	response::success(ListCertsResponse {
		certificates: cert_map,
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
