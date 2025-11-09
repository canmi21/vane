/* src/modules/ports/handler.rs */

use super::model::{PortState, Protocol};
use crate::common::{getconf, portool};
use crate::core::response;
use crate::modules::stack::transport::fs;
use axum::{
	extract::{Path, State},
	http::StatusCode,
	response::{IntoResponse, Response},
};
use std::fs as std_fs; // Renamed to avoid conflict with our fs module

/// Handles GET /ports - Lists all configured port numbers from the filesystem.
pub async fn get_ports_handler() -> Response {
	let config_dir = getconf::get_config_dir();
	let mut ports = Vec::new();

	let entries = match std_fs::read_dir(&config_dir) {
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
	match std_fs::create_dir(&port_dir) {
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
	match std_fs::remove_dir_all(&port_dir) {
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

	// UPDATED: Call the function from its new location in l4::fs
	match fs::create_protocol_listener(port, &protocol) {
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

	// UPDATED: Call the function from its new location in l4::fs
	match fs::delete_protocol_listener(port, &protocol) {
		Ok(_) => (StatusCode::NO_CONTENT).into_response(),
		Err(e) => response::error(
			StatusCode::INTERNAL_SERVER_ERROR,
			format!("Failed to delete listener config: {}", e),
		)
		.into_response(),
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::modules::ports::model::{PortState, PortStatus};
	use arc_swap::ArcSwap;
	use axum::{
		Router,
		body::Body,
		http::{Request, StatusCode},
		routing::{delete, get, post},
	};
	use serde_json::Value;
	use std::sync::Arc;
	use tower::util::ServiceExt;

	/// Helper to build a test router with the necessary state and handlers.
	fn build_test_router(state: PortState) -> Router {
		Router::new()
			.route("/ports", get(get_ports_handler))
			.route("/ports/{port}", get(get_port_status_handler))
			.route("/ports/{port}", post(post_port_handler))
			.route("/ports/{port}", delete(delete_port_handler))
			.route("/ports/{port}/{protocol}", post(post_protocol_handler))
			.route("/ports/{port}/{protocol}", delete(delete_protocol_handler))
			.with_state(state)
	}

	/// Helper to deserialize the body of a response into JSON.
	async fn get_body_as_json(response: Response) -> Value {
		let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
			.await
			.unwrap();
		if body_bytes.is_empty() {
			return Value::Null;
		}
		serde_json::from_slice(&body_bytes).unwrap()
	}

	/// Tests the handler for getting a port's live status from memory.
	#[tokio::test]
	async fn test_get_port_status_from_state() {
		// Prepare a mock in-memory state.
		let mock_status = PortStatus {
			port: 9090,
			active: true,
			tcp_config: None, // Simplified for this test
			udp_config: None,
		};
		let state = Arc::new(ArcSwap::new(Arc::new(vec![mock_status])));
		let app = build_test_router(state);

		// 1. Request a port that exists in the state.
		let req = Request::builder()
			.uri("/ports/9090")
			.body(Body::empty())
			.unwrap();
		let res = app.clone().oneshot(req).await.unwrap();
		assert_eq!(res.status(), StatusCode::OK);
		let body = get_body_as_json(res).await;
		assert_eq!(body["data"]["port"], 9090);

		// 2. Request a port that does NOT exist in the state.
		let req = Request::builder()
			.uri("/ports/1234")
			.body(Body::empty())
			.unwrap();
		let res = app.clone().oneshot(req).await.unwrap();
		assert_eq!(res.status(), StatusCode::NOT_FOUND);
	}
}
