/* engine/src/modules/layout/manager.rs */

use crate::{
	common::response, daemon::config, modules::domain::entrance as domain_helper,
	proxy::router::generate::generate_router_tree,
};
use axum::{
	Json,
	extract::Path,
	http::StatusCode,
	response::{IntoResponse, Response},
};
use fancy_log::{LogLevel, log};
use serde_json::Value;
use std::path::{Path as FilePath, PathBuf};

// --- Helper Functions ---

/// Gets the full path to a domain's specific layout.json file.
fn get_layout_config_path(domain: &str) -> PathBuf {
	let domain_dir_name = domain_helper::domain_to_dir_name(domain);
	config::get_config_dir()
		.join(domain_dir_name)
		.join("layout.json")
}

/// Ensures a layout.json file exists for a domain, creating an empty one if not.
// --- FIX: Made this function public so it can be called from the domain module. ---
pub async fn ensure_layout_config_exists(domain_dir: &FilePath) {
	let layout_path = domain_dir.join("layout.json");
	if !layout_path.exists() {
		if let Some(dir_name) = domain_dir.file_name().and_then(|s| s.to_str()) {
			if let Err(e) = tokio::fs::write(&layout_path, "{}").await {
				log(
					LogLevel::Error,
					&format!(
						"Failed to create default layout.json for {}: {}",
						dir_name, e
					),
				);
			} else {
				log(
					LogLevel::Debug,
					&format!("+ Created default layout.json for {}", dir_name),
				);
			}
		}
	}
}

/// Scans all existing domain directories and ensures each has a layout.json file.
/// This function is intended to be called once on application startup.
pub async fn initialize_all_layout_configs() {
	log(LogLevel::Info, "Checking for layout.json in all domains...");
	let config_dir = config::get_config_dir();
	let domains = domain_helper::list_domains_internal().await;

	for domain_name in domains {
		let domain_dir = config_dir.join(domain_helper::domain_to_dir_name(&domain_name));
		if domain_dir.is_dir() {
			ensure_layout_config_exists(&domain_dir).await;
		}
	}
	log(LogLevel::Info, "Layout configuration check complete.");
}

// --- Axum Handlers ---

/// Retrieves the layout configuration for a specific domain.
pub async fn get_layout_config(Path(domain): Path<String>) -> Response {
	log(
		LogLevel::Debug,
		&format!("GET /v1/layout/{} called", domain),
	);
	let domain_dir = config::get_config_dir().join(domain_helper::domain_to_dir_name(&domain));
	if !domain_dir.exists() {
		return response::error(StatusCode::NOT_FOUND, "Domain not found.".to_string()).into_response();
	}

	let path = get_layout_config_path(&domain);
	match tokio::fs::read_to_string(&path).await {
		Ok(content) => match serde_json::from_str::<Value>(&content) {
			Ok(json_value) => response::success(json_value).into_response(),
			Err(e) => {
				log(
					LogLevel::Error,
					&format!("Failed to parse layout.json for {}: {}", domain, e),
				);
				response::error(
					StatusCode::INTERNAL_SERVER_ERROR,
					"Failed to parse layout configuration.".to_string(),
				)
				.into_response()
			}
		},
		Err(e) => {
			log(
				LogLevel::Error,
				&format!("Failed to read layout.json for {}: {}", domain, e),
			);
			response::error(
				StatusCode::INTERNAL_SERVER_ERROR,
				"Failed to read layout configuration.".to_string(),
			)
			.into_response()
		}
	}
}

/// Updates the layout configuration for a specific domain and regenerates the router tree.
pub async fn update_layout_config(
	Path(domain): Path<String>,
	Json(payload): Json<Value>,
) -> Response {
	log(LogLevel::Info, &format!("PUT /v1/layout/{} called", domain));
	let domain_dir = config::get_config_dir().join(domain_helper::domain_to_dir_name(&domain));
	if !domain_dir.exists() {
		return response::error(StatusCode::NOT_FOUND, "Domain not found.".to_string()).into_response();
	}

	let path = get_layout_config_path(&domain);
	match serde_json::to_string_pretty(&payload) {
		Ok(content) => {
			if let Err(e) = tokio::fs::write(&path, content).await {
				log(
					LogLevel::Error,
					&format!("Failed to write to layout.json for {}: {}", domain, e),
				);
				return response::error(
					StatusCode::INTERNAL_SERVER_ERROR,
					"Failed to save layout configuration.".to_string(),
				)
				.into_response();
			}

			// Trigger router tree regeneration after successful update.
			generate_router_tree(&domain).await;

			response::success(payload).into_response()
		}
		Err(_) => {
			response::error(StatusCode::BAD_REQUEST, "Invalid JSON payload.".to_string()).into_response()
		}
	}
}
