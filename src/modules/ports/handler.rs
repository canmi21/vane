/* src/modules/ports/handler.rs */

use super::{
	hotswap,
	model::{PortState, Protocol},
};
use axum::{
	extract::{Path, State},
	http::StatusCode,
	response::{IntoResponse, Response},
};
use std::fs;

use crate::common::{getconf, portool};
use crate::core::response;

/// Handles GET /ports - Lists all configured port numbers from the filesystem.
pub async fn get_ports_handler() -> Response {
	let config_dir = getconf::get_config_dir();
	let mut ports = Vec::new();

	let entries = match fs::read_dir(&config_dir) {
		Ok(entries) => entries,
		Err(e) => {
			return response::error(
				StatusCode::INTERNAL_SERVER_ERROR,
				format!("Failed to read config directory: {}", e),
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

/// Handles GET /ports/{:port} - Shows the live in-memory status of a single port.
pub async fn get_port_status_handler(
	State(state): State<PortState>,
	Path(port): Path<u16>,
) -> Response {
	let state_guard = state.load();
	let port_status = state_guard.iter().find(|p| p.port == port);

	match port_status {
		Some(status) => response::success(status).into_response(),
		None => response::error(
			StatusCode::NOT_FOUND,
			"Port configuration not found.".to_string(),
		)
		.into_response(),
	}
}

/// Handles POST /ports/{:port} - Creates a new port configuration directory.
pub async fn post_port_handler(Path(port): Path<u16>) -> Response {
	if !portool::is_valid_port(port) {
		return response::error(StatusCode::BAD_REQUEST, "Invalid port number.".to_string())
			.into_response();
	}
	let port_dir = getconf::get_config_dir().join(format!("[{}]", port));
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

/// Handles DELETE /ports/{:port} - Deletes a port configuration directory.
pub async fn delete_port_handler(Path(port): Path<u16>) -> Response {
	let port_dir = getconf::get_config_dir().join(format!("[{}]", port));
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

/// Handles POST /ports/{:port}/{:protocol} - Adds a protocol listener to a port.
pub async fn post_protocol_handler(Path((port, protocol_str)): Path<(u16, String)>) -> Response {
	let protocol = match protocol_str.as_str() {
		"tcp" => Protocol::Tcp,
		"udp" => Protocol::Udp,
		_ => {
			return response::error(
				StatusCode::BAD_REQUEST,
				"Invalid protocol: must be 'tcp' or 'udp'.".to_string(),
			)
			.into_response();
		}
	};

	match hotswap::create_protocol_listener(port, &protocol) {
		Ok(_) => (StatusCode::CREATED).into_response(),
		Err(e) => response::error(
			StatusCode::INTERNAL_SERVER_ERROR,
			format!("Failed to create listener config: {}", e),
		)
		.into_response(),
	}
}

/// Handles DELETE /ports/{:port}/{:protocol} - Removes a protocol listener from a port.
pub async fn delete_protocol_handler(Path((port, protocol_str)): Path<(u16, String)>) -> Response {
	let protocol = match protocol_str.as_str() {
		"tcp" => Protocol::Tcp,
		"udp" => Protocol::Udp,
		_ => {
			return response::error(
				StatusCode::BAD_REQUEST,
				"Invalid protocol: must be 'tcp' or 'udp'.".to_string(),
			)
			.into_response();
		}
	};

	match hotswap::delete_protocol_listener(port, &protocol) {
		Ok(_) => (StatusCode::NO_CONTENT).into_response(),
		Err(e) => response::error(
			StatusCode::INTERNAL_SERVER_ERROR,
			format!("Failed to delete listener config: {}", e),
		)
		.into_response(),
	}
}
