/* engine/src/modules/domain/entrance.rs */

use crate::{common::response, daemon::config};
use axum::extract::Path;
use axum::response::IntoResponse;
use axum::{http::StatusCode, response::Response};
use fancy_log::{LogLevel, log};
use serde::{Deserialize, Serialize};

// --- Helper Functions ---

/// Converts a user-facing domain name (like "*.example.com") to a filesystem-safe directory name ("[_.example.com]").
pub fn domain_to_dir_name(domain: &str) -> String {
	let fs_safe_name = domain.replace('*', "_");
	format!("[{}]", fs_safe_name)
}

/// Converts a directory name ("[_.example.com]") back to a user-facing domain name ("*.example.com").
/// Returns None if the directory name is not in the expected format.
pub fn dir_name_to_domain(dir_name: &str) -> Option<String> {
	if dir_name.starts_with('[') && dir_name.ends_with(']') {
		let inner = &dir_name[1..dir_name.len() - 1];
		Some(inner.replace('_', "*"))
	} else {
		None
	}
}

/// Validates if a given string is a plausible domain name for our use case.
/// Checks for dot count and basic structural validity.
pub fn is_valid_domain_input(domain: &str) -> bool {
	if domain.is_empty() || domain.len() > 253 {
		return false;
	}
	// Limit the depth of subdomains.
	if domain.matches('.').count() > 32 {
		return false;
	}

	// Handle the wildcard case by checking the part after it.
	let domain_to_check = if let Some(stripped) = domain.strip_prefix("*.") {
		if stripped.contains('*') {
			return false; // Only one wildcard at the start is allowed.
		}
		stripped
	} else {
		domain
	};

	// A very basic check for valid characters.
	domain_to_check
		.split('.')
		.all(|label| !label.is_empty() && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
}

// --- API Payloads ---

// This struct is no longer needed for create/delete but might be useful elsewhere.
#[derive(Deserialize, Serialize)]
pub struct DomainPayload {
	pub domain: String,
}

#[derive(Serialize)]
pub struct ListDomainsResponse {
	pub domains: Vec<String>,
}

// --- Axum Handlers ---

/// Lists all configured domain entrances by scanning the config directory.
pub async fn list_domains() -> Response {
	log(LogLevel::Debug, "GET /v1/domains called");
	let config_path = config::get_config_dir();
	let mut domains = Vec::new();

	let mut entries = match tokio::fs::read_dir(config_path).await {
		Ok(entries) => entries,
		Err(e) => {
			log(
				LogLevel::Error,
				&format!("Failed to read config directory: {}", e),
			);
			return response::error(
				StatusCode::INTERNAL_SERVER_ERROR,
				"Could not read configuration directory.".to_string(),
			)
			.into_response();
		}
	};

	while let Ok(Some(entry)) = entries.next_entry().await {
		if let Ok(file_type) = entry.file_type().await {
			if file_type.is_dir() {
				let dir_name = entry.file_name().to_string_lossy().to_string();
				// Ignore the special [fallback] directory.
				if dir_name == "[fallback]" {
					continue;
				}
				if let Some(domain) = dir_name_to_domain(&dir_name) {
					domains.push(domain);
				}
			}
		}
	}

	domains.sort(); // For consistent ordering.
	response::success(ListDomainsResponse { domains }).into_response()
}

/// Creates a new domain entrance by creating a corresponding directory.
pub async fn create_domain(Path(domain): Path<String>) -> Response {
	log(
		LogLevel::Info,
		&format!("POST /v1/domains/{} called", domain),
	);

	if !is_valid_domain_input(&domain) {
		return response::error(
			StatusCode::BAD_REQUEST,
			"Invalid domain format provided.".to_string(),
		)
		.into_response();
	}

	let dir_name = domain_to_dir_name(&domain);
	let mut path = config::get_config_dir();
	path.push(&dir_name);

	if path.exists() {
		return response::error(StatusCode::CONFLICT, "Domain already exists.".to_string())
			.into_response();
	}

	if let Err(e) = tokio::fs::create_dir(&path).await {
		log(
			LogLevel::Error,
			&format!("Failed to create domain directory '{}': {}", dir_name, e),
		);
		return response::error(
			StatusCode::INTERNAL_SERVER_ERROR,
			"Failed to create domain directory.".to_string(),
		)
		.into_response();
	}

	log(
		LogLevel::Info,
		&format!("Domain entrance created: {}", domain),
	);
	// --- Respond with the created domain info ---
	(
		StatusCode::CREATED,
		response::success(DomainPayload { domain }),
	)
		.into_response()
}

/// Deletes a domain entrance by removing its directory.
pub async fn delete_domain(Path(domain): Path<String>) -> Response {
	log(
		LogLevel::Info,
		&format!("DELETE /v1/domains/{} called", domain),
	);

	if !is_valid_domain_input(&domain) {
		return response::error(
			StatusCode::BAD_REQUEST,
			"Invalid domain format provided.".to_string(),
		)
		.into_response();
	}

	let dir_name = domain_to_dir_name(&domain);
	let mut path = config::get_config_dir();
	path.push(&dir_name);

	if !path.exists() {
		return response::error(StatusCode::NOT_FOUND, "Domain not found.".to_string()).into_response();
	}

	if let Err(e) = tokio::fs::remove_dir_all(&path).await {
		log(
			LogLevel::Error,
			&format!("Failed to delete domain directory '{}': {}", dir_name, e),
		);
		return response::error(
			StatusCode::INTERNAL_SERVER_ERROR,
			"Failed to delete domain directory.".to_string(),
		)
		.into_response();
	}

	log(
		LogLevel::Info,
		&format!("Domain entrance deleted: {}", domain),
	);
	StatusCode::NO_CONTENT.into_response()
}
