/* engine/src/modules/cache/manager.rs */

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

// --- Data Structures for cache.json ---

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct CacheRule {
	pub path: String,
	pub ttl_seconds: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CacheConfig {
	// If true, honors Cache-Control headers from the origin.
	pub respect_origin_cache_control: bool,
	// Specific caching rules for different paths.
	pub path_rules: Vec<CacheRule>,
	// Paths that should never be cached.
	pub blacklist_paths: Vec<String>,
}

impl Default for CacheConfig {
	/// Defines the default cache configuration.
	fn default() -> Self {
		Self {
			respect_origin_cache_control: true,
			path_rules: Vec::new(),
			blacklist_paths: Vec::new(),
		}
	}
}

// --- API Payloads ---

#[derive(Deserialize, Serialize)]
pub struct PathPayload {
	pub path: String,
}

// --- Helper Functions ---

/// Gets the full path to a domain's specific cache.json file.
pub fn get_cache_config_path(domain: &str) -> PathBuf {
	let domain_dir_name = domain_helper::domain_to_dir_name(domain);
	config::get_config_dir()
		.join(domain_dir_name)
		.join("cache.json")
}

/// Reads and deserializes the cache.json file for a given domain.
pub async fn load_cache_config(domain: &str) -> Result<CacheConfig, String> {
	let path = get_cache_config_path(domain);
	if !path.exists() {
		return Err("Cache config file not found.".to_string());
	}
	let content = tokio::fs::read_to_string(&path)
		.await
		.map_err(|e| format!("Failed to read cache.json: {}", e))?;
	serde_json::from_str(&content).map_err(|e| format!("Failed to parse cache.json: {}", e))
}

/// Serializes and writes a CacheConfig to the appropriate cache.json file.
pub async fn save_cache_config(domain: &str, config: &CacheConfig) -> Result<(), String> {
	let path = get_cache_config_path(domain);
	let contents = serde_json::to_string_pretty(config)
		.map_err(|e| format!("Failed to serialize Cache config: {}", e))?;
	tokio::fs::write(&path, contents)
		.await
		.map_err(|e| format!("Failed to write cache.json: {}", e))
}

/// Ensures a cache.json file exists for a domain, creating a default one if not.
pub async fn ensure_cache_config_exists(domain_dir: &FilePath) {
	let cache_path = domain_dir.join("cache.json");
	if !cache_path.exists() {
		if let Some(dir_name) = domain_dir.file_name().and_then(|s| s.to_str()) {
			let default_config = CacheConfig::default();
			let contents = serde_json::to_string_pretty(&default_config).unwrap();
			if let Err(e) = tokio::fs::write(&cache_path, contents).await {
				log(
					LogLevel::Error,
					&format!(
						"Failed to create default cache.json for {}: {}",
						dir_name, e
					),
				);
			} else {
				log(
					LogLevel::Debug,
					&format!("+ Created default cache.json for {}", dir_name),
				);
			}
		}
	}
}

// --- Axum Handlers ---

/// Retrieves the detailed Cache configuration for a specific domain.
pub async fn get_cache_config(Path(domain): Path<String>) -> Response {
	log(LogLevel::Debug, &format!("GET /v1/cache/{} called", domain));

	let domain_dir = config::get_config_dir().join(domain_helper::domain_to_dir_name(&domain));
	if !domain_dir.exists() {
		return response::error(StatusCode::NOT_FOUND, "Domain not found.".to_string()).into_response();
	}

	ensure_cache_config_exists(&domain_dir).await;

	match load_cache_config(&domain).await {
		Ok(config) => response::success(config).into_response(),
		Err(e) => response::error(StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
	}
}

/// Updates the entire Cache configuration for a specific domain.
pub async fn update_cache_config(
	Path(domain): Path<String>,
	Json(payload): Json<CacheConfig>,
) -> Response {
	log(LogLevel::Info, &format!("PUT /v1/cache/{} called", domain));

	if let Err(e) = save_cache_config(&domain, &payload).await {
		return response::error(StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
	}

	response::success(payload).into_response()
}

/// Resets the Cache configuration for a domain to its default state.
pub async fn reset_cache_config(Path(domain): Path<String>) -> Response {
	log(
		LogLevel::Info,
		&format!("DELETE /v1/cache/{} called", domain),
	);

	let default_config = CacheConfig::default();
	if let Err(e) = save_cache_config(&domain, &default_config).await {
		return response::error(StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
	}

	(StatusCode::OK, "Cache configuration reset to default.").into_response()
}

/// Adds or updates a specific path-based cache rule.
pub async fn add_or_update_cache_rule(
	Path(domain): Path<String>,
	Json(payload): Json<CacheRule>,
) -> Response {
	log(
		LogLevel::Info,
		&format!("POST /v1/cache/{}/rules called", domain),
	);

	let mut config = match load_cache_config(&domain).await {
		Ok(c) => c,
		Err(_) => {
			return response::error(
				StatusCode::NOT_FOUND,
				"Domain configuration not found.".to_string(),
			)
			.into_response();
		}
	};

	// If a rule for this path already exists, update it. Otherwise, add it.
	if let Some(rule) = config
		.path_rules
		.iter_mut()
		.find(|r| r.path == payload.path)
	{
		rule.ttl_seconds = payload.ttl_seconds;
	} else {
		config.path_rules.push(payload);
	}

	if let Err(e) = save_cache_config(&domain, &config).await {
		return response::error(StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
	}

	response::success(config).into_response()
}

/// Removes a specific path-based cache rule.
pub async fn remove_cache_rule(
	Path(domain): Path<String>,
	Json(payload): Json<PathPayload>,
) -> Response {
	log(
		LogLevel::Info,
		&format!("DELETE /v1/cache/{}/rules called", domain),
	);

	let mut config = match load_cache_config(&domain).await {
		Ok(c) => c,
		Err(_) => {
			return response::error(
				StatusCode::NOT_FOUND,
				"Domain configuration not found.".to_string(),
			)
			.into_response();
		}
	};

	let initial_len = config.path_rules.len();
	config.path_rules.retain(|r| r.path != payload.path);

	if config.path_rules.len() == initial_len {
		return response::error(
			StatusCode::NOT_FOUND,
			"Cache rule for the specified path not found.".to_string(),
		)
		.into_response();
	}

	if let Err(e) = save_cache_config(&domain, &config).await {
		return response::error(StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
	}

	response::success(config).into_response()
}

/// Adds a path to the cache blacklist.
pub async fn add_blacklist_path(
	Path(domain): Path<String>,
	Json(payload): Json<PathPayload>,
) -> Response {
	log(
		LogLevel::Info,
		&format!("POST /v1/cache/{}/blacklist called", domain),
	);

	let mut config = match load_cache_config(&domain).await {
		Ok(c) => c,
		Err(_) => {
			return response::error(
				StatusCode::NOT_FOUND,
				"Domain configuration not found.".to_string(),
			)
			.into_response();
		}
	};

	if config.blacklist_paths.contains(&payload.path) {
		return response::error(
			StatusCode::CONFLICT,
			"Path already exists in blacklist.".to_string(),
		)
		.into_response();
	}

	config.blacklist_paths.push(payload.path);

	if let Err(e) = save_cache_config(&domain, &config).await {
		return response::error(StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
	}

	response::success(config).into_response()
}

/// Removes a path from the cache blacklist.
pub async fn remove_blacklist_path(
	Path(domain): Path<String>,
	Json(payload): Json<PathPayload>,
) -> Response {
	log(
		LogLevel::Info,
		&format!("DELETE /v1/cache/{}/blacklist called", domain),
	);

	let mut config = match load_cache_config(&domain).await {
		Ok(c) => c,
		Err(_) => {
			return response::error(
				StatusCode::NOT_FOUND,
				"Domain configuration not found.".to_string(),
			)
			.into_response();
		}
	};

	let initial_len = config.blacklist_paths.len();
	config.blacklist_paths.retain(|p| p != &payload.path);

	if config.blacklist_paths.len() == initial_len {
		return response::error(
			StatusCode::NOT_FOUND,
			"Path not found in blacklist.".to_string(),
		)
		.into_response();
	}

	if let Err(e) = save_cache_config(&domain, &config).await {
		return response::error(StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
	}

	response::success(config).into_response()
}
