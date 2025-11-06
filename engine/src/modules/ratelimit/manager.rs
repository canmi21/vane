/* engine/src/modules/ratelimit/manager.rs */

use crate::{common::response, daemon::config, proxy::domain::handler as domain_helper};
use axum::{
	Json,
	extract::Path,
	http::StatusCode,
	response::{IntoResponse, Response},
};
use fancy_log::{LogLevel, log};
use serde::{Deserialize, Serialize};
use std::path::{Path as FilePath, PathBuf};

// --- Data Structure for ratelimit.json ---

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RateLimitConfig {
	// Maximum requests per second. A value of 0 disables rate limiting.
	pub requests_per_second: u32,
}

impl Default for RateLimitConfig {
	/// Defines the default rate limit configuration, which is disabled.
	fn default() -> Self {
		Self {
			requests_per_second: 0,
		}
	}
}

// --- Helper Functions (all public) ---

/// Gets the full path to a domain's specific ratelimit.json file.
pub fn get_ratelimit_config_path(domain: &str) -> PathBuf {
	let domain_dir_name = domain_helper::domain_to_dir_name(domain);
	config::get_config_dir()
		.join(domain_dir_name)
		.join("ratelimit.json")
}

/// Reads and deserializes the ratelimit.json file for a given domain.
pub async fn load_ratelimit_config(domain: &str) -> Result<RateLimitConfig, String> {
	let path = get_ratelimit_config_path(domain);
	if !path.exists() {
		return Err("Rate limit config file not found.".to_string());
	}
	let content = tokio::fs::read_to_string(&path)
		.await
		.map_err(|e| format!("Failed to read ratelimit.json: {}", e))?;
	serde_json::from_str(&content).map_err(|e| format!("Failed to parse ratelimit.json: {}", e))
}

/// Serializes and writes a RateLimitConfig to the appropriate ratelimit.json file.
pub async fn save_ratelimit_config(domain: &str, config: &RateLimitConfig) -> Result<(), String> {
	let path = get_ratelimit_config_path(domain);
	let contents = serde_json::to_string_pretty(config)
		.map_err(|e| format!("Failed to serialize RateLimit config: {}", e))?;
	tokio::fs::write(&path, contents)
		.await
		.map_err(|e| format!("Failed to write ratelimit.json: {}", e))
}

/// Ensures a ratelimit.json file exists for a domain, creating a default one if not.
pub async fn ensure_ratelimit_config_exists(domain_dir: &FilePath) {
	let ratelimit_path = domain_dir.join("ratelimit.json");
	if !ratelimit_path.exists() {
		if let Some(dir_name) = domain_dir.file_name().and_then(|s| s.to_str()) {
			let default_config = RateLimitConfig::default();
			let contents = serde_json::to_string_pretty(&default_config).unwrap();
			if let Err(e) = tokio::fs::write(&ratelimit_path, contents).await {
				log(
					LogLevel::Error,
					&format!(
						"Failed to create default ratelimit.json for {}: {}",
						dir_name, e
					),
				);
			} else {
				log(
					LogLevel::Debug,
					&format!("+ Created default ratelimit.json for {}", dir_name),
				);
			}
		}
	}
}

// --- Axum Handlers ---

/// Retrieves the detailed RateLimit configuration for a specific domain.
pub async fn get_ratelimit_config(Path(domain): Path<String>) -> Response {
	log(
		LogLevel::Debug,
		&format!("GET /v1/ratelimit/{} called", domain),
	);

	let domain_dir = config::get_config_dir().join(domain_helper::domain_to_dir_name(&domain));
	if !domain_dir.exists() {
		return response::error(StatusCode::NOT_FOUND, "Domain not found.".to_string()).into_response();
	}

	ensure_ratelimit_config_exists(&domain_dir).await;

	match load_ratelimit_config(&domain).await {
		Ok(config) => response::success(config).into_response(),
		Err(e) => response::error(StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
	}
}

/// Updates the RateLimit configuration for a specific domain.
pub async fn update_ratelimit_config(
	Path(domain): Path<String>,
	Json(payload): Json<RateLimitConfig>,
) -> Response {
	log(
		LogLevel::Info,
		&format!("PUT /v1/ratelimit/{} called", domain),
	);

	if let Err(e) = save_ratelimit_config(&domain, &payload).await {
		return response::error(StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
	}

	response::success(payload).into_response()
}

/// Resets the RateLimit configuration for a domain to its default state.
pub async fn reset_ratelimit_config(Path(domain): Path<String>) -> Response {
	log(
		LogLevel::Info,
		&format!("DELETE /v1/ratelimit/{} called", domain),
	);

	let default_config = RateLimitConfig::default();
	if let Err(e) = save_ratelimit_config(&domain, &default_config).await {
		return response::error(StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
	}

	(StatusCode::OK, "Rate limit configuration reset to default.").into_response()
}
