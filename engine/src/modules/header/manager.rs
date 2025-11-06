/* engine/src/modules/header/manager.rs */

use crate::{common::response, daemon::config, proxy::domain::handler as domain_helper};
use axum::{
	Json,
	extract::Path,
	http::StatusCode,
	response::{IntoResponse, Response},
};
use fancy_log::{LogLevel, log};
use serde::{Deserialize, Serialize};
use std::{
	collections::HashMap,
	path::{Path as FilePath, PathBuf},
};

// --- Data Structure for header.json ---

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct HeaderConfig {
	// A flexible map to store any header key-value pairs.
	pub headers: HashMap<String, String>,
}

impl Default for HeaderConfig {
	/// Defines the default response headers.
	fn default() -> Self {
		let mut headers = HashMap::new();
		headers.insert("Server".to_string(), "Self-Host".to_string());
		Self { headers }
	}
}

// --- Helper Functions (all public) ---

/// Gets the full path to a domain's specific header.json file.
pub fn get_header_config_path(domain: &str) -> PathBuf {
	let domain_dir_name = domain_helper::domain_to_dir_name(domain);
	config::get_config_dir()
		.join(domain_dir_name)
		.join("header.json")
}

/// Reads and deserializes the header.json file for a given domain.
pub async fn load_header_config(domain: &str) -> Result<HeaderConfig, String> {
	let path = get_header_config_path(domain);
	if !path.exists() {
		return Err("Header config file not found.".to_string());
	}
	let content = tokio::fs::read_to_string(&path)
		.await
		.map_err(|e| format!("Failed to read header.json: {}", e))?;
	serde_json::from_str(&content).map_err(|e| format!("Failed to parse header.json: {}", e))
}

/// Serializes and writes a HeaderConfig to the appropriate header.json file.
pub async fn save_header_config(domain: &str, config: &HeaderConfig) -> Result<(), String> {
	let path = get_header_config_path(domain);
	let contents = serde_json::to_string_pretty(config)
		.map_err(|e| format!("Failed to serialize Header config: {}", e))?;
	tokio::fs::write(&path, contents)
		.await
		.map_err(|e| format!("Failed to write header.json: {}", e))
}

/// Ensures a header.json file exists for a domain, creating a default one if not.
pub async fn ensure_header_config_exists(domain_dir: &FilePath) {
	let header_path = domain_dir.join("header.json");
	if !header_path.exists() {
		if let Some(dir_name) = domain_dir.file_name().and_then(|s| s.to_str()) {
			let default_config = HeaderConfig::default();
			let contents = serde_json::to_string_pretty(&default_config).unwrap();
			if let Err(e) = tokio::fs::write(&header_path, contents).await {
				log(
					LogLevel::Error,
					&format!(
						"Failed to create default header.json for {}: {}",
						dir_name, e
					),
				);
			} else {
				log(
					LogLevel::Debug,
					&format!("+ Created default header.json for {}", dir_name),
				);
			}
		}
	}
}

// --- Axum Handlers ---

/// Retrieves the detailed Header configuration for a specific domain.
pub async fn get_header_config(Path(domain): Path<String>) -> Response {
	log(
		LogLevel::Debug,
		&format!("GET /v1/headers/{} called", domain),
	);

	let domain_dir = config::get_config_dir().join(domain_helper::domain_to_dir_name(&domain));
	if !domain_dir.exists() {
		return response::error(StatusCode::NOT_FOUND, "Domain not found.".to_string()).into_response();
	}

	ensure_header_config_exists(&domain_dir).await;

	match load_header_config(&domain).await {
		Ok(config) => response::success(config).into_response(),
		Err(e) => response::error(StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
	}
}

/// Updates the Header configuration for a specific domain.
pub async fn update_header_config(
	Path(domain): Path<String>,
	Json(payload): Json<HeaderConfig>,
) -> Response {
	log(
		LogLevel::Info,
		&format!("PUT /v1/headers/{} called", domain),
	);

	// Basic validation: ensure header names and values are not empty.
	for (key, value) in &payload.headers {
		if key.trim().is_empty() || value.trim().is_empty() {
			return response::error(
				StatusCode::BAD_REQUEST,
				"Header names and values cannot be empty.".to_string(),
			)
			.into_response();
		}
	}

	if let Err(e) = save_header_config(&domain, &payload).await {
		return response::error(StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
	}

	response::success(payload).into_response()
}

/// Resets the Header configuration for a domain to its default state.
pub async fn reset_header_config(Path(domain): Path<String>) -> Response {
	log(
		LogLevel::Info,
		&format!("DELETE /v1/headers/{} called", domain),
	);

	let default_config = HeaderConfig::default();
	if let Err(e) = save_header_config(&domain, &default_config).await {
		return response::error(StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
	}

	(StatusCode::OK, "Header configuration reset to default.").into_response()
}
