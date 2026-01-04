/* src/ingress/handler.rs */

use super::model::{PortState, Protocol};
use crate::api::response;
use crate::common::{config::getconf, net::portool};
use crate::layers::l4::fs as transport_fs;
use axum::{
	extract::{Path, State},
	http::StatusCode,
	response::{IntoResponse, Response},
};
use tokio::fs;

pub async fn get_ports_handler() -> Response {
	let config_dir = getconf::get_config_dir();
	let mut ports = Vec::new();

	let mut entries = match fs::read_dir(&config_dir).await {
		Ok(entries) => entries,
		Err(e) => {
			return response::error(
				StatusCode::INTERNAL_SERVER_ERROR,
				format!("Failed to read config directory: {}", e),
			)
			.into_response();
		}
	};

	while let Ok(Some(entry)) = entries.next_entry().await {
		if let Ok(metadata) = entry.metadata().await {
			if !metadata.is_dir() {
				continue;
			}
		} else {
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

pub async fn get_port_status_handler(
	State(state): State<PortState>,
	Path(port): Path<u16>,
) -> Response {
	let state_guard = state.load();
	let port_status = state_guard.iter().find(|p| p.port == port);
	match port_status {
		Some(status) => response::success(status).into_response(),
		None => response::error(StatusCode::NOT_FOUND, "Port not found.".into()).into_response(),
	}
}

pub async fn post_port_handler(Path(port): Path<u16>) -> Response {
	if !portool::is_valid_port(port) {
		return response::error(StatusCode::BAD_REQUEST, "Invalid port.".into()).into_response();
	}
	let port_dir = getconf::get_config_dir().join(format!("[{}]", port));
	if fs::metadata(&port_dir).await.is_ok() {
		return response::error(StatusCode::CONFLICT, "Exists.".into()).into_response();
	}
	match fs::create_dir(&port_dir).await {
		Ok(_) => (StatusCode::CREATED).into_response(),
		Err(e) => response::error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
	}
}

pub async fn delete_port_handler(Path(port): Path<u16>) -> Response {
	let port_dir = getconf::get_config_dir().join(format!("[{}]", port));
	if fs::metadata(&port_dir).await.is_err() {
		return response::error(StatusCode::NOT_FOUND, "Not found.".into()).into_response();
	}
	match fs::remove_dir_all(&port_dir).await {
		Ok(_) => (StatusCode::NO_CONTENT).into_response(),
		Err(e) => response::error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
	}
}

pub async fn post_protocol_handler(Path((port, protocol_str)): Path<(u16, String)>) -> Response {
	let protocol = match protocol_str.as_str() {
		"tcp" => Protocol::Tcp,
		"udp" => Protocol::Udp,
		_ => return response::error(StatusCode::BAD_REQUEST, "Invalid proto.".into()).into_response(),
	};
	match transport_fs::create_protocol_listener(port, &protocol).await {
		Ok(_) => (StatusCode::CREATED).into_response(),
		Err(e) => response::error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
	}
}

pub async fn delete_protocol_handler(Path((port, protocol_str)): Path<(u16, String)>) -> Response {
	let protocol = match protocol_str.as_str() {
		"tcp" => Protocol::Tcp,
		"udp" => Protocol::Udp,
		_ => return response::error(StatusCode::BAD_REQUEST, "Invalid proto.".into()).into_response(),
	};
	match transport_fs::delete_protocol_listener(port, &protocol).await {
		Ok(_) => (StatusCode::NO_CONTENT).into_response(),
		Err(e) => response::error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
	}
}
