/* src/api/handlers/system.rs */

use crate::response;
use crate::schemas::system::{
	BuildInfo, HealthStatus, HealthStatusResponse, PackageInfo, RuntimeInfo, SystemInfo,
	SystemInfoResponse, SystemStatusDetails, SystemStatusResponse,
};
use axum::response::IntoResponse;
use std::env;
use vane_engine::registry;
use vane_primitives::certs::arcswap as cert_registry;
use vane_primitives::common::sys::lifecycle;

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
pub async fn status_handler() -> impl IntoResponse {
	let config = vane_engine::config::get();

	// 1. Listeners
	let tcp_map = config.listeners.tcp.snapshot().await;
	let udp_map = config.listeners.udp.snapshot().await;

	let mut tcp_ports: Vec<u16> = tcp_map.keys().filter_map(|k| k.parse().ok()).collect();
	let mut udp_ports: Vec<u16> = udp_map.keys().filter_map(|k| k.parse().ok()).collect();

	tcp_ports.sort();
	udp_ports.sort();

	let mut unique_ports = std::collections::HashSet::<u16>::new();
	unique_ports.extend(tcp_ports.iter());
	unique_ports.extend(udp_ports.iter());

	// 2. Plugins
	let internal_count = registry::list_internal_plugins().len();
	let external_count = registry::list_external_plugins().len();
	let healthy_external = registry::EXTERNAL_PLUGIN_STATUS
		.iter()
		.filter(|r| r.value().is_ok())
		.count();

	// 3. Resources
	// Nodes
	let nodes_count = config.nodes.get().map(|n| n.nodes.len()).unwrap_or(0);

	// Certs (Legacy for now)
	let certs_count = cert_registry::CERT_REGISTRY.len();

	// Resolvers (from memory)
	let resolvers_count = config.resolvers.len().await;

	// Applications (from memory)
	let apps_count = config.applications.len().await;

	response::success(SystemStatusDetails {
		listeners: serde_json::json!({
			"active": unique_ports.len(),
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
