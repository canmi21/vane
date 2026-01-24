/* src/api/handlers/system.rs */

use crate::api::response;
use crate::api::schemas::system::{
	BuildInfo, HealthStatus, HealthStatusResponse, PackageInfo, RuntimeInfo, SystemInfo,
	SystemInfoResponse, SystemStatusDetails, SystemStatusResponse,
};
use crate::common::config::file_loader;
use crate::common::sys::lifecycle;
use crate::ingress::state::PortState;
use crate::plugins::core::registry;
use crate::resources::certs::arcswap as cert_registry;
use crate::resources::service_discovery::model::NODES_STATE;
use axum::extract::State;
use axum::response::IntoResponse;
use std::env;
use tokio::fs;

// --- Handlers ---

/// Get system information
#[utoipa::path(
    get,
    path = "/system",
    responses(
        (status = 200, description = "System information", body = SystemInfoResponse)
    ),
    tag = "system"
)]
pub async fn root_handler() -> impl IntoResponse {
	let pkg_name = env!("CARGO_PKG_NAME").to_owned();
	let pkg_version = env!("CARGO_PKG_VERSION").to_owned();
	let repository = env!("CARGO_PKG_REPOSITORY").to_owned();
	let license = env!("CARGO_PKG_LICENSE").to_owned();

	let git_commit = env!("GIT_COMMIT_SHORT").to_owned();
	let rustc_version = env!("RUSTC_FULL_VERSION").to_owned();
	let cargo_version = env!("CARGO_FULL_VERSION").to_owned();
	let build_date = env!("BUILD_DATE").to_owned();

	let arch = env::consts::ARCH.to_owned();
	let os = env::consts::OS.to_owned();

	let info = SystemInfo {
		package: PackageInfo {
			name: pkg_name,
			version: pkg_version,
			author: "Canmi(Canmi21) t@canmi.icu".to_owned(),
			license,
			repository,
		},
		build: BuildInfo {
			rust_version: rustc_version,
			cargo_version,
			build_date,
			git_commit,
		},
		runtime: RuntimeInfo { arch, platform: os },
	};

	response::success(info)
}

/// Health check
#[utoipa::path(
    get,
    path = "/health",
    responses(
        (status = 200, description = "System is healthy", body = HealthStatusResponse)
    ),
    tag = "system"
)]
pub async fn health_handler() -> impl IntoResponse {
	let uptime = std::time::Instant::now()
		.duration_since(*lifecycle::START_TIME)
		.as_secs();

	response::success(HealthStatus {
		healthy: true,
		uptime_secs: uptime,
	})
}

/// Detailed system status
#[utoipa::path(
    get,
    path = "/status",
    responses(
        (status = 200, description = "Detailed system status", body = SystemStatusResponse),
        (status = 401, description = "Unauthorized")
    ),
    security(
        ("bearer_auth" = [])
    ),
    tag = "system"
)]
pub async fn status_handler(State(state): State<PortState>) -> impl IntoResponse {
	// 1. Listeners
	let state_guard = state.load();
	let mut tcp_ports = Vec::new();
	let mut udp_ports = Vec::new();
	for p in state_guard.iter() {
		if p.tcp_config.is_some() {
			tcp_ports.push(p.port);
		}
		if p.udp_config.is_some() {
			udp_ports.push(p.port);
		}
	}

	// 2. Plugins
	let internal_count = registry::list_internal_plugins().len();
	let external_count = registry::list_external_plugins().len();
	let healthy_external = registry::EXTERNAL_PLUGIN_STATUS
		.iter()
		.filter(|r| r.value().is_ok())
		.count();

	// 3. Resources
	let nodes_count = NODES_STATE.load().nodes.len();
	let certs_count = cert_registry::CERT_REGISTRY.load().len();

	let config_dir = file_loader::get_config_dir();
	let mut resolvers_count = 0;
	if let Ok(mut entries) = fs::read_dir(config_dir.join("resolvers")).await {
		while let Ok(Some(_)) = entries.next_entry().await {
			resolvers_count += 1;
		}
	}
	let mut apps_count = 0;
	if let Ok(mut entries) = fs::read_dir(config_dir.join("applications")).await {
		while let Ok(Some(_)) = entries.next_entry().await {
			apps_count += 1;
		}
	}

	response::success(SystemStatusDetails {
		listeners: serde_json::json!({
			"active": state_guard.len(),
			"tcp": tcp_ports,
			"udp": udp_ports
		}),
		plugins: serde_json::json!({
			"internal": internal_count,
			"external": external_count,
			"external_healthy": healthy_external
		}),
		resources: serde_json::json!({
			"nodes": nodes_count,
			"certificates": certs_count,
			"resolvers": resolvers_count,
			"applications": apps_count
		}),
	})
}
