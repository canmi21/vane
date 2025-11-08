/* src/modules/ports/handler.rs */

use axum::{
	extract::Path,
	http::StatusCode,
	response::{IntoResponse, Response},
};
use std::fs;

use crate::common::{getconf, portool};
use crate::core::response;

/// GET /ports - Lists all configured listener ports.
pub async fn get_ports_handler() -> Response {
	let config_dir = getconf::get_config_dir();
	let mut ports = Vec::new();

	let entries = match fs::read_dir(&config_dir) {
		Ok(entries) => entries,
		Err(e) => {
			return response::error(
				StatusCode::INTERNAL_SERVER_ERROR,
				format!(
					"Failed to read config directory {}: {}",
					config_dir.display(),
					e
				),
			)
			.into_response();
		}
	};

	for entry in entries.flatten() {
		if !entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
			continue;
		}

		if let Some(name) = entry.file_name().to_str() {
			if name.starts_with('[') && name.ends_with(']') {
				if let Ok(port) = name[1..name.len() - 1].parse::<u16>() {
					ports.push(port);
				}
			}
		}
	}

	ports.sort_unstable();
	response::success(ports).into_response()
}

/// POST /ports/{port} - Creates a new listener port configuration.
pub async fn post_port_handler(Path(port): Path<u16>) -> Response {
	if !portool::is_valid_port(port) {
		return response::error(StatusCode::BAD_REQUEST, "Invalid port number.".to_string())
			.into_response();
	}

	let config_dir = getconf::get_config_dir();
	let port_dir = config_dir.join(format!("[{}]", port));

	if port_dir.exists() {
		return response::error(
			StatusCode::CONFLICT,
			"Port configuration already exists.".to_string(),
		)
		.into_response();
	}

	match fs::create_dir(&port_dir) {
		Ok(_) => (StatusCode::CREATED).into_response(),
		Err(e) => response::error(
			StatusCode::INTERNAL_SERVER_ERROR,
			format!("Failed to create port directory: {}", e),
		)
		.into_response(),
	}
}

/// DELETE /ports/{port} - Deletes a listener port configuration.
pub async fn delete_port_handler(Path(port): Path<u16>) -> Response {
	let config_dir = getconf::get_config_dir();
	let port_dir = config_dir.join(format!("[{}]", port));

	if !port_dir.exists() {
		return response::error(
			StatusCode::NOT_FOUND,
			"Port configuration not found.".to_string(),
		)
		.into_response();
	}

	match fs::remove_dir_all(&port_dir) {
		Ok(_) => (StatusCode::NO_CONTENT).into_response(),
		Err(e) => response::error(
			StatusCode::INTERNAL_SERVER_ERROR,
			format!("Failed to delete port directory: {}", e),
		)
		.into_response(),
	}
}
