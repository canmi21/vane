/* engine/src/modules/websocket/manager.rs */

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

// --- Data Structure for websocket.json ---

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct WebSocketConfig {
	// Whether to proxy WebSocket upgrade requests.
	pub enabled: bool,
	// A list of paths to listen for WebSocket upgrades. "*" for all paths.
	pub paths: Vec<String>,
}

impl Default for WebSocketConfig {
	/// Defines the default WebSocket configuration, which is disabled.
	fn default() -> Self {
		Self {
			enabled: false,
			paths: vec!["*".to_string()],
		}
	}
}

// --- API Payloads ---

#[derive(Deserialize, Serialize)]
pub struct PathPayload {
	pub path: String,
}

// --- Helper Functions (all public) ---

/// Gets the full path to a domain's specific websocket.json file.
pub fn get_websocket_config_path(domain: &str) -> PathBuf {
	let domain_dir_name = domain_helper::domain_to_dir_name(domain);
	config::get_config_dir()
		.join(domain_dir_name)
		.join("websocket.json")
}

/// Reads and deserializes the websocket.json file for a given domain.
pub async fn load_websocket_config(domain: &str) -> Result<WebSocketConfig, String> {
	let path = get_websocket_config_path(domain);
	if !path.exists() {
		return Err("WebSocket config file not found.".to_string());
	}
	let content = tokio::fs::read_to_string(&path)
		.await
		.map_err(|e| format!("Failed to read websocket.json: {}", e))?;
	serde_json::from_str(&content).map_err(|e| format!("Failed to parse websocket.json: {}", e))
}

/// Serializes and writes a WebSocketConfig to the appropriate websocket.json file.
pub async fn save_websocket_config(domain: &str, config: &WebSocketConfig) -> Result<(), String> {
	let path = get_websocket_config_path(domain);
	let contents = serde_json::to_string_pretty(config)
		.map_err(|e| format!("Failed to serialize WebSocket config: {}", e))?;
	tokio::fs::write(&path, contents)
		.await
		.map_err(|e| format!("Failed to write websocket.json: {}", e))
}

/// Ensures a websocket.json file exists for a domain, creating a default one if not.
pub async fn ensure_websocket_config_exists(domain_dir: &FilePath) {
	let websocket_path = domain_dir.join("websocket.json");
	if !websocket_path.exists() {
		if let Some(dir_name) = domain_dir.file_name().and_then(|s| s.to_str()) {
			let default_config = WebSocketConfig::default();
			let contents = serde_json::to_string_pretty(&default_config).unwrap();
			if let Err(e) = tokio::fs::write(&websocket_path, contents).await {
				log(
					LogLevel::Error,
					&format!(
						"Failed to create default websocket.json for {}: {}",
						dir_name, e
					),
				);
			} else {
				log(
					LogLevel::Debug,
					&format!("+ Created default websocket.json for {}", dir_name),
				);
			}
		}
	}
}

// --- Axum Handlers ---

/// Retrieves the detailed WebSocket configuration for a specific domain.
pub async fn get_websocket_config(Path(domain): Path<String>) -> Response {
	log(
		LogLevel::Debug,
		&format!("GET /v1/websocket/{} called", domain),
	);

	let domain_dir = config::get_config_dir().join(domain_helper::domain_to_dir_name(&domain));
	if !domain_dir.exists() {
		return response::error(StatusCode::NOT_FOUND, "Domain not found.".to_string()).into_response();
	}

	ensure_websocket_config_exists(&domain_dir).await;

	match load_websocket_config(&domain).await {
		Ok(config) => response::success(config).into_response(),
		Err(e) => response::error(StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
	}
}

/// Updates the entire WebSocket configuration for a specific domain.
pub async fn update_websocket_config(
	Path(domain): Path<String>,
	Json(payload): Json<WebSocketConfig>,
) -> Response {
	log(
		LogLevel::Info,
		&format!("PUT /v1/websocket/{} called", domain),
	);

	// Basic validation: paths cannot contain empty strings.
	if payload.paths.iter().any(|p| p.trim().is_empty()) {
		return response::error(
			StatusCode::BAD_REQUEST,
			"WebSocket paths cannot be empty.".to_string(),
		)
		.into_response();
	}
	// If the list is empty, it means no paths are allowed, which is a valid state.

	if let Err(e) = save_websocket_config(&domain, &payload).await {
		return response::error(StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
	}

	response::success(payload).into_response()
}

/// Resets the WebSocket configuration for a domain to its default state.
pub async fn reset_websocket_config(Path(domain): Path<String>) -> Response {
	log(
		LogLevel::Info,
		&format!("DELETE /v1/websocket/{} called", domain),
	);

	let default_config = WebSocketConfig::default();
	if let Err(e) = save_websocket_config(&domain, &default_config).await {
		return response::error(StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
	}

	(StatusCode::OK, "WebSocket configuration reset to default.").into_response()
}

// --- NEW PATH-SPECIFIC HANDLERS ---

/// Adds a new WebSocket proxy path for a domain.
pub async fn add_websocket_path(
	Path(domain): Path<String>,
	Json(payload): Json<PathPayload>,
) -> Response {
	log(
		LogLevel::Info,
		&format!("POST /v1/websocket/{}/paths called", domain),
	);

	if payload.path.trim().is_empty() {
		return response::error(StatusCode::BAD_REQUEST, "Path cannot be empty.".to_string())
			.into_response();
	}

	let mut config = match load_websocket_config(&domain).await {
		Ok(c) => c,
		Err(_) => {
			return response::error(
				StatusCode::NOT_FOUND,
				"Domain configuration not found.".to_string(),
			)
			.into_response();
		}
	};

	if config.paths.contains(&payload.path) {
		return response::error(StatusCode::CONFLICT, "Path already exists.".to_string())
			.into_response();
	}

	config.paths.push(payload.path);

	if let Err(e) = save_websocket_config(&domain, &config).await {
		return response::error(StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
	}

	response::success(config).into_response()
}

/// Removes a WebSocket proxy path from a domain.
pub async fn remove_websocket_path(
	Path(domain): Path<String>,
	Json(payload): Json<PathPayload>,
) -> Response {
	log(
		LogLevel::Info,
		&format!("DELETE /v1/websocket/{}/paths called", domain),
	);

	let mut config = match load_websocket_config(&domain).await {
		Ok(c) => c,
		Err(_) => {
			return response::error(
				StatusCode::NOT_FOUND,
				"Domain configuration not found.".to_string(),
			)
			.into_response();
		}
	};

	let initial_len = config.paths.len();
	config.paths.retain(|p| p != &payload.path);

	if config.paths.len() == initial_len {
		return response::error(
			StatusCode::NOT_FOUND,
			"Path not found in configuration.".to_string(),
		)
		.into_response();
	}

	if let Err(e) = save_websocket_config(&domain, &config).await {
		return response::error(StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
	}

	response::success(config).into_response()
}
