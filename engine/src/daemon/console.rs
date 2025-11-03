/* engine/src/daemon/console.rs */

use crate::common::response;
use crate::daemon::config;
use axum::{
	Json,
	http::StatusCode,
	response::{IntoResponse, Response},
};
use fancy_log::{LogLevel, log};
use serde_json::Value;

/// Initializes the console configuration file.
/// If `console.json` does not exist or is empty, it creates it with `{}`.
pub async fn initialize_console_config() {
	let path = config::get_console_config_path();
	let file_exists = tokio::fs::try_exists(&path).await.unwrap_or(false);

	let needs_initialization = if file_exists {
		// If file exists, check if it's empty.
		match tokio::fs::read_to_string(&path).await {
			Ok(content) => content.trim().is_empty(),
			Err(_) => true, // Error reading, treat as needing initialization.
		}
	} else {
		true // File doesn't exist, needs initialization.
	};

	if needs_initialization {
		log(
			LogLevel::Debug,
			"console.json not found or is empty. Initializing.",
		);
		match tokio::fs::write(&path, "{}").await {
			Ok(_) => log(
				LogLevel::Info,
				&format!("Successfully created empty console.json at {:?}", path),
			),
			Err(e) => log(
				LogLevel::Error,
				&format!("Failed to create initial console.json: {}", e),
			),
		}
	}
}

/// Retrieves the contents of console.json.
pub async fn get_console_config() -> Response {
	log(LogLevel::Debug, "GET /v1/console called");
	let path = config::get_console_config_path();

	match tokio::fs::read_to_string(&path).await {
		Ok(content) => match serde_json::from_str::<Value>(&content) {
			Ok(json_value) => response::success(json_value).into_response(),
			Err(e) => {
				log(
					LogLevel::Error,
					&format!("Failed to parse console.json: {}", e),
				);
				response::error(
					StatusCode::INTERNAL_SERVER_ERROR,
					"Failed to parse console configuration.".to_string(),
				)
				.into_response()
			}
		},
		Err(e) => {
			log(
				LogLevel::Error,
				&format!("Failed to read console.json: {}", e),
			);
			response::error(
				StatusCode::INTERNAL_SERVER_ERROR,
				"Failed to read console configuration.".to_string(),
			)
			.into_response()
		}
	}
}

/// Updates the contents of console.json with the provided payload.
pub async fn update_console_config(Json(payload): Json<Value>) -> Response {
	log(LogLevel::Info, "PUT /v1/console called");
	let path = config::get_console_config_path();

	match serde_json::to_string_pretty(&payload) {
		Ok(content) => {
			if let Err(e) = tokio::fs::write(&path, content).await {
				log(
					LogLevel::Error,
					&format!("Failed to write to console.json: {}", e),
				);
				return response::error(
					StatusCode::INTERNAL_SERVER_ERROR,
					"Failed to save console configuration.".to_string(),
				)
				.into_response();
			}
			log(
				LogLevel::Info,
				"Console configuration updated successfully.",
			);
			response::success(payload).into_response()
		}
		Err(e) => {
			log(
				LogLevel::Error,
				&format!("Failed to serialize console config payload: {}", e),
			);
			response::error(StatusCode::BAD_REQUEST, "Invalid JSON payload.".to_string()).into_response()
		}
	}
}
