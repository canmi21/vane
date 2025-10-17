/* engine/src/modules/cors/manager.rs */

use crate::{common::response, daemon::config, modules::domain::entrance as domain_helper};
use axum::{
	Json,
	extract::Path,
	http::StatusCode,
	response::{IntoResponse, Response},
};
use fancy_log::{LogLevel, log};
use serde::{Deserialize, Serialize};
use std::path::{Path as FilePath, PathBuf};

// --- Data Structures for cors.json ---

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PreflightHandling {
	ProxyDecision,  // Vane handles the OPTIONS request based on this config.
	OriginResponse, // Vane passes the OPTIONS request to the origin server.
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CorsConfig {
	pub preflight_handling: PreflightHandling,
	pub allow_origins: Vec<String>,
	pub allow_methods: Vec<String>,
	pub allow_headers: Vec<String>,
	pub allow_credentials: bool,
	pub expose_headers: Vec<String>,
	pub max_age_seconds: u64,
}

impl Default for CorsConfig {
	/// Defines the default CORS configuration, which is to pass everything to the origin.
	fn default() -> Self {
		Self {
			preflight_handling: PreflightHandling::OriginResponse,
			allow_origins: vec!["*".to_string()],
			allow_methods: vec!["*".to_string()],
			allow_headers: vec![],
			allow_credentials: false,
			expose_headers: vec![],
			max_age_seconds: 3600,
		}
	}
}

// --- Helper Functions (all public) ---

/// Gets the full path to a domain's specific cors.json file.
pub fn get_cors_config_path(domain: &str) -> PathBuf {
	let domain_dir_name = domain_helper::domain_to_dir_name(domain);
	config::get_config_dir()
		.join(domain_dir_name)
		.join("cors.json")
}

/// Reads and deserializes the cors.json file for a given domain.
pub async fn load_cors_config(domain: &str) -> Result<CorsConfig, String> {
	let path = get_cors_config_path(domain);
	if !path.exists() {
		return Err("CORS config file not found.".to_string());
	}
	let content = tokio::fs::read_to_string(&path)
		.await
		.map_err(|e| format!("Failed to read cors.json: {}", e))?;
	serde_json::from_str(&content).map_err(|e| format!("Failed to parse cors.json: {}", e))
}

/// Serializes and writes a CorsConfig to the appropriate cors.json file.
pub async fn save_cors_config(domain: &str, config: &CorsConfig) -> Result<(), String> {
	let path = get_cors_config_path(domain);
	let contents = serde_json::to_string_pretty(config)
		.map_err(|e| format!("Failed to serialize CORS config: {}", e))?;
	tokio::fs::write(&path, contents)
		.await
		.map_err(|e| format!("Failed to write cors.json: {}", e))
}

/// Ensures a cors.json file exists for a domain, creating a default one if not.
pub async fn ensure_cors_config_exists(domain_dir: &FilePath) {
	let cors_path = domain_dir.join("cors.json");
	if !cors_path.exists() {
		if let Some(dir_name) = domain_dir.file_name().and_then(|s| s.to_str()) {
			let default_config = CorsConfig::default();
			let contents = serde_json::to_string_pretty(&default_config).unwrap();
			if let Err(e) = tokio::fs::write(&cors_path, contents).await {
				log(
					LogLevel::Error,
					&format!("Failed to create default cors.json for {}: {}", dir_name, e),
				);
			} else {
				log(
					LogLevel::Debug,
					&format!("+ Created default cors.json for {}", dir_name),
				);
			}
		}
	}
}

// --- API Payloads ---

#[derive(Serialize)]
pub struct CorsStatus {
	pub domain: String,
	pub preflight_handling: PreflightHandling,
}

// --- Axum Handlers ---

/// Lists all domains and their current CORS preflight handling status.
pub async fn list_cors_status() -> Response {
	log(LogLevel::Debug, "GET /v1/cors called");
	let config_path = config::get_config_dir();
	let mut statuses = Vec::new();

	let mut entries = match tokio::fs::read_dir(config_path).await {
		Ok(e) => e,
		Err(_) => {
			return response::error(
				StatusCode::INTERNAL_SERVER_ERROR,
				"Could not read config directory.".to_string(),
			)
			.into_response();
		}
	};

	while let Ok(Some(entry)) = entries.next_entry().await {
		let path = entry.path();
		if path.is_dir() {
			if let Some(dir_name) = path.file_name().and_then(|s| s.to_str()) {
				if let Some(domain) = domain_helper::dir_name_to_domain(dir_name) {
					// Ensure config exists before trying to read it.
					ensure_cors_config_exists(&path).await;

					let config = load_cors_config(&domain).await.unwrap_or_default();
					statuses.push(CorsStatus {
						domain,
						preflight_handling: config.preflight_handling,
					});
				} else if dir_name == "[fallback]" {
					ensure_cors_config_exists(&path).await;
					let config = load_cors_config("fallback").await.unwrap_or_default();
					statuses.push(CorsStatus {
						domain: "fallback".to_string(),
						preflight_handling: config.preflight_handling,
					});
				}
			}
		}
	}

	response::success(statuses).into_response()
}

/// Retrieves the detailed CORS configuration for a specific domain.
pub async fn get_cors_config(Path(domain): Path<String>) -> Response {
	log(LogLevel::Debug, &format!("GET /v1/cors/{} called", domain));

	// Ensure the domain/fallback directory itself exists first.
	let domain_dir = config::get_config_dir().join(domain_helper::domain_to_dir_name(&domain));
	if !domain_dir.exists() {
		return response::error(StatusCode::NOT_FOUND, "Domain not found.".to_string()).into_response();
	}

	ensure_cors_config_exists(&domain_dir).await;

	match load_cors_config(&domain).await {
		Ok(config) => response::success(config).into_response(),
		Err(e) => response::error(StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
	}
}

/// Updates the CORS configuration for a specific domain.
pub async fn update_cors_config(
	Path(domain): Path<String>,
	Json(payload): Json<CorsConfig>,
) -> Response {
	log(LogLevel::Info, &format!("PUT /v1/cors/{} called", domain));

	// Validation logic.
	if payload.allow_credentials && payload.allow_origins.contains(&"*".to_string()) {
		return response::error(
			StatusCode::BAD_REQUEST,
			"Cannot allow credentials with wildcard origin ('*').".to_string(),
		)
		.into_response();
	}

	if payload.preflight_handling == PreflightHandling::ProxyDecision {
		let has_options = payload.allow_methods.contains(&"*".to_string())
			|| payload
				.allow_methods
				.iter()
				.any(|m| m.eq_ignore_ascii_case("OPTIONS"));
		if !has_options {
			return response::error(
				StatusCode::BAD_REQUEST,
				"ProxyDecision requires 'OPTIONS' to be in allow_methods, or use '*'.".to_string(),
			)
			.into_response();
		}
	}

	if let Err(e) = save_cors_config(&domain, &payload).await {
		return response::error(StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
	}

	response::success(payload).into_response()
}

/// Resets the CORS configuration for a domain to its default state.
pub async fn reset_cors_config(Path(domain): Path<String>) -> Response {
	log(
		LogLevel::Info,
		&format!("DELETE /v1/cors/{} called", domain),
	);

	let default_config = CorsConfig::default();
	if let Err(e) = save_cors_config(&domain, &default_config).await {
		return response::error(StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
	}

	(StatusCode::OK, "CORS configuration reset to default.").into_response()
}
