/* src/api/handlers/ports.rs */

use crate::api::response;
use crate::api::schemas::ports::{
	PortCreated, PortCreatedResponse, PortDetail, PortDetailResponse, PortInfo, PortListResponse,
	ProtocolStatus,
};
use crate::api::utils::config_file;
use crate::common::{config::file_loader, net::port_utils};
use crate::ingress::state::{PortState, Protocol};
use crate::layers::l4::fs as transport_fs;
use axum::{
	extract::{Path, State},
	http::StatusCode,
	response::IntoResponse,
};
use tokio::fs;

// --- Handlers ---

/// List all ports
#[utoipa::path(
    get,
    path = "/ports",
    responses(
        (status = 200, description = "List of ports", body = PortListResponse)
    ),
    tag = "ports",
    security(("bearer_auth" = []))
)]
pub async fn list_ports_handler(State(state): State<PortState>) -> impl IntoResponse {
	let config_dir = file_loader::get_config_dir();
	let mut ports = Vec::new();

	// Read from filesystem (source of truth for configuration)
	let mut entries = match fs::read_dir(&config_dir).await {
		Ok(entries) => entries,
		Err(e) => {
			return response::error(
				StatusCode::INTERNAL_SERVER_ERROR,
				format!("Failed to read config directory: {e}"),
			);
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

		if let Some(name) = entry.file_name().to_str()
			&& name.starts_with('[')
			&& name.ends_with(']')
			&& let Ok(port_num) = name[1..name.len() - 1].parse::<u16>()
		{
			// Check active state from runtime memory
			let state_guard = state.load();
			let status = state_guard.iter().find(|p| p.port == port_num);
			let active = status.is_some();

			let mut protocols = Vec::new();
			let tcp_dir = config_dir.join(name).join("tcp");
			if fs::metadata(&tcp_dir).await.is_ok() {
				protocols.push("tcp".to_owned());
			}
			let udp_dir = config_dir.join(name).join("udp");
			if fs::metadata(&udp_dir).await.is_ok() {
				protocols.push("udp".to_owned());
			}

			ports.push(PortInfo {
				port: port_num,
				protocols,
				active,
			});
		}
	}

	ports.sort_by_key(|p| p.port);
	response::success(ports)
}

/// Get port details
#[utoipa::path(
    get,
    path = "/ports/{port}",
    params(
        ("port" = u16, Path, description = "Port number")
    ),
    responses(
        (status = 200, description = "Port details", body = PortDetailResponse),
        (status = 404, description = "Port not configured")
    ),
    tag = "ports",
    security(("bearer_auth" = []))
)]
pub async fn get_port_handler(
	State(state): State<PortState>,
	Path(port): Path<u16>,
) -> impl IntoResponse {
	let port_dir = file_loader::get_config_dir().join(format!("[{port}]"));
	if fs::metadata(&port_dir).await.is_err() {
		return response::error(StatusCode::NOT_FOUND, format!("Port {port} not configured"));
	}

	// Runtime status
	let state_guard = state.load();
	let runtime_status = state_guard.iter().find(|p| p.port == port);

	// TCP Info
	let tcp_path = port_dir.join("tcp");
	let tcp = if fs::metadata(&tcp_path).await.is_ok() {
		let config_res = config_file::find_config::<serde_json::Value>(&tcp_path).await;
		let source_format = match config_res {
			config_file::ConfigFileResult::Single { format, .. } => Some(format),
			_ => None,
		};

		let active = runtime_status
			.map(|s| s.tcp_config.is_some())
			.unwrap_or(false);

		Some(ProtocolStatus {
			active,
			source_format,
		})
	} else {
		None
	};

	// UDP Info
	let udp_path = port_dir.join("udp");
	let udp = if fs::metadata(&udp_path).await.is_ok() {
		let config_res = config_file::find_config::<serde_json::Value>(&udp_path).await;
		let source_format = match config_res {
			config_file::ConfigFileResult::Single { format, .. } => Some(format),
			_ => None,
		};

		let active = runtime_status
			.map(|s| s.udp_config.is_some())
			.unwrap_or(false);

		Some(ProtocolStatus {
			active,
			source_format,
		})
	} else {
		None
	};

	response::success(PortDetail { port, tcp, udp })
}

/// Create port
#[utoipa::path(
    post,
    path = "/ports/{port}",
    params(
        ("port" = u16, Path, description = "Port number")
    ),
    responses(
        (status = 201, description = "Port created", body = PortCreatedResponse),
        (status = 400, description = "Invalid port"),
        (status = 409, description = "Port already exists")
    ),
    tag = "ports",
    security(("bearer_auth" = []))
)]
pub async fn create_port_handler(Path(port): Path<u16>) -> impl IntoResponse {
	if !port_utils::is_valid_port(port) {
		return response::error(StatusCode::BAD_REQUEST, "Invalid port number".into());
	}
	let port_dir = file_loader::get_config_dir().join(format!("[{port}]"));
	if fs::metadata(&port_dir).await.is_ok() {
		return response::error(StatusCode::CONFLICT, format!("Port {port} already exists"));
	}
	match fs::create_dir(&port_dir).await {
		Ok(_) => response::created(PortCreated {
			port,
			created: true,
		}),
		Err(e) => response::error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
	}
}

/// Delete port
#[utoipa::path(
    delete,
    path = "/ports/{port}",
    params(
        ("port" = u16, Path, description = "Port number")
    ),
    responses(
        (status = 204, description = "Port deleted"),
        (status = 404, description = "Port not found")
    ),
    tag = "ports",
    security(("bearer_auth" = []))
)]
pub async fn delete_port_handler(Path(port): Path<u16>) -> impl IntoResponse {
	let port_dir = file_loader::get_config_dir().join(format!("[{port}]"));
	if fs::metadata(&port_dir).await.is_err() {
		return response::error(StatusCode::NOT_FOUND, format!("Port {port} not found"));
	}
	match fs::remove_dir_all(&port_dir).await {
		Ok(_) => StatusCode::NO_CONTENT.into_response(),
		Err(e) => response::error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
	}
}

// Note: create_protocol_listener and delete_protocol_listener were in the router
// but strict adhering to API Spec might suggest managing them via the flow endpoint creation implicitly?
// However, the spec lists `POST /ports/{port}` but not explicit `POST /ports/{port}/{protocol}`
// except for Flow.
// Let's check spec again.
// Spec says: POST /ports/{port} creates the directory.
// Flow endpoints manipulate the config files.
// The intermediate directory `tcp/` or `udp/` inside `[port]/` needs to be created.
// It seems `POST /ports/{port}/{protocol}/flow` handles creating the file.
// But we might need explicit endpoints for creating protocol dirs if they are separate?
// Looking at `src/ingress/api.rs`, `post_protocol_handler` creates the directory listener.
// In the new design, `post_flow_handler` writes the file.
// If the parent directory (e.g., `tcp`) doesn't exist, `write_json` might fail or we should ensure it exists.
// Let's keep `post_protocol_handler` (create dir) logic, but maybe it's implicitly handled?
// Wait, `transport_fs::create_protocol_listener` does more than just mkdir, it triggers hot swap watcher logic?
// No, watcher watches the root.
// Let's keep the explicit protocol management for now, but API Spec didn't explicitly list `POST /ports/{port}/{protocol}` as a public API.
// It listed `POST /ports/{port}/{protocol}/flow`.
// If I assume `POST flow` creates the config, does it also ensure the protocol is enabled?
// Yes, `find_config` checks for file existence.
// So `POST /ports/{port}/{protocol}` might be redundant if we just use Flow API.
// BUT, the directory `tcp` or `udp` must exist for the watcher to scan it?
// Vane's loader logic: `[8080]/tcp/config.json`.
// So yes, `tcp` is a directory.

// Let's implement `POST /ports/{port}/{protocol}` as "Enable Protocol" (mkdir)
// and `DELETE` as "Disable Protocol" (rmdir).

/// Enable protocol for port
#[utoipa::path(
    post,
    path = "/ports/{port}/{protocol}",
    params(
        ("port" = u16, Path, description = "Port number"),
        ("protocol" = String, Path, description = "Protocol (tcp/udp)")
    ),
    responses(
        (status = 201, description = "Protocol enabled"),
        (status = 400, description = "Invalid protocol")
    ),
    tag = "ports",
    security(("bearer_auth" = []))
)]
pub async fn enable_protocol_handler(
	Path((port, protocol_str)): Path<(u16, String)>,
) -> impl IntoResponse {
	let protocol = match protocol_str.as_str() {
		"tcp" => Protocol::Tcp,
		"udp" => Protocol::Udp,
		_ => return response::error(StatusCode::BAD_REQUEST, "Invalid protocol".into()),
	};

	// We use the existing logic from transport_fs to create the directory
	match transport_fs::create_protocol_listener(port, &protocol).await {
		Ok(_) => StatusCode::CREATED.into_response(),
		Err(e) => response::error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
	}
}

/// Disable protocol for port
#[utoipa::path(
    delete,
    path = "/ports/{port}/{protocol}",
    params(
        ("port" = u16, Path, description = "Port number"),
        ("protocol" = String, Path, description = "Protocol (tcp/udp)")
    ),
    responses(
        (status = 204, description = "Protocol disabled"),
        (status = 400, description = "Invalid protocol")
    ),
    tag = "ports",
    security(("bearer_auth" = []))
)]
pub async fn disable_protocol_handler(
	Path((port, protocol_str)): Path<(u16, String)>,
) -> impl IntoResponse {
	let protocol = match protocol_str.as_str() {
		"tcp" => Protocol::Tcp,
		"udp" => Protocol::Udp,
		_ => return response::error(StatusCode::BAD_REQUEST, "Invalid protocol".into()),
	};
	match transport_fs::delete_protocol_listener(port, &protocol).await {
		Ok(_) => StatusCode::NO_CONTENT.into_response(),
		Err(e) => response::error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
	}
}
