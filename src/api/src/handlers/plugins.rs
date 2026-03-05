/* src/api/handlers/plugins.rs */

use crate::response;
use crate::schemas::plugins::{
	ListPluginsQuery, ParamDefResponse, PluginDetail, PluginDetailResponse, PluginList,
	PluginListResponse, PluginOperationResponse, PluginOperationResult, PluginSummary,
};
use axum::{
	Json,
	extract::{Path, Query},
	http::StatusCode,
	response::IntoResponse,
};
use std::sync::Arc;
use vane_engine::engine::interfaces::{ExternalPluginConfig, Plugin};
use vane_engine::registry;
use vane_extra::core::loader;

// --- Helpers ---

fn map_plugin_summary(plugin: &Arc<dyn Plugin>, is_internal: bool) -> PluginSummary {
	let role = if plugin.as_terminator().is_some() || plugin.as_l7_terminator().is_some() {
		"terminator"
	} else {
		"middleware"
	};

	let healthy = if !is_internal {
		Some(registry::EXTERNAL_PLUGIN_STATUS.get(plugin.name()).map(|r| r.is_ok()).unwrap_or(true))
	} else {
		None
	};

	PluginSummary {
		name: plugin.name().to_owned(),
		role: role.to_owned(),
		type_name: if is_internal { "internal" } else { "external" }.to_owned(),
		healthy,
	}
}

fn map_plugin_detail(plugin: &Arc<dyn Plugin>, is_internal: bool) -> PluginDetail {
	let role = if plugin.as_terminator().is_some() || plugin.as_l7_terminator().is_some() {
		"terminator"
	} else {
		"middleware"
	};

	let params = plugin
		.params()
		.into_iter()
		.map(|p| ParamDefResponse {
			name: p.name.to_string(),
			required: p.required,
			param_type: format!("{:?}", p.param_type),
		})
		.collect();

	let supported_protocols =
		plugin.supported_protocols().into_iter().map(|p| p.to_string()).collect();

	let healthy = if !is_internal {
		Some(registry::EXTERNAL_PLUGIN_STATUS.get(plugin.name()).map(|r| r.is_ok()).unwrap_or(true))
	} else {
		None
	};

	PluginDetail {
		name: plugin.name().to_owned(),
		type_name: if is_internal { "internal" } else { "external" }.to_owned(),
		role: role.to_owned(),
		params,
		supported_protocols,
		driver: None,
		healthy,
	}
}

// --- Handlers ---

/// List all plugins
#[utoipa::path(
    get,
    path = "/plugins",
    params(ListPluginsQuery),
    responses(
        (status = 200, description = "List of plugins", body = PluginListResponse)
    ),
    tag = "plugins",
    security(("bearer_auth" = []))
)]
pub async fn list_plugins_handler(Query(query): Query<ListPluginsQuery>) -> impl IntoResponse {
	let show_internal = matches!(query.type_name.as_deref(), None | Some("all" | "internal"));
	let show_external = matches!(query.type_name.as_deref(), None | Some("all" | "external"));

	let internal = if show_internal {
		registry::list_internal_plugins().iter().map(|p| map_plugin_summary(p, true)).collect()
	} else {
		vec![]
	};

	let external = if show_external {
		registry::list_external_plugins().iter().map(|p| map_plugin_summary(p, false)).collect()
	} else {
		vec![]
	};

	response::success(PluginList { internal, external })
}

/// Get plugin details
#[utoipa::path(
    get,
    path = "/plugins/{name}",
    params(
        ("name" = String, Path, description = "Plugin name")
    ),
    responses(
        (status = 200, description = "Plugin details", body = PluginDetailResponse),
        (status = 404, description = "Plugin not found")
    ),
    tag = "plugins",
    security(("bearer_auth" = []))
)]
pub async fn get_plugin_handler(Path(name): Path<String>) -> impl IntoResponse {
	if let Some(plugin) = registry::get_internal_plugin(&name) {
		return response::success(map_plugin_detail(&plugin, true));
	}

	if let Some(plugin) = registry::get_external_plugin(&name) {
		return response::success(map_plugin_detail(&plugin, false));
	}

	response::error(StatusCode::NOT_FOUND, format!("Plugin '{name}' not found"))
}

/// Register external plugin
#[utoipa::path(
    post,
    path = "/plugins/{name}",
    params(
        ("name" = String, Path, description = "Plugin name")
    ),
    request_body = ExternalPluginConfig,
    responses(
        (status = 201, description = "Plugin registered", body = PluginOperationResponse),
        (status = 400, description = "Invalid request"),
        (status = 409, description = "Plugin already exists")
    ),
    tag = "plugins",
    security(("bearer_auth" = []))
)]
pub async fn create_plugin_handler(
	Path(name): Path<String>,
	Json(config): Json<ExternalPluginConfig>,
) -> impl IntoResponse {
	if config.name != name {
		return response::error(
			StatusCode::BAD_REQUEST,
			"Path name and body name mismatch.".to_owned(),
		);
	}

	if registry::get_plugin(&name).is_some() {
		return response::error(StatusCode::CONFLICT, format!("Plugin '{name}' already exists."));
	}

	match loader::register_plugin(config).await {
		Ok(_) => response::created(PluginOperationResult { status: "created".into(), name }),
		Err(e) => response::error(StatusCode::BAD_REQUEST, format!("Failed to register plugin: {e}")),
	}
}

/// Update external plugin
#[utoipa::path(
    put,
    path = "/plugins/{name}",
    params(
        ("name" = String, Path, description = "Plugin name")
    ),
    request_body = ExternalPluginConfig,
    responses(
        (status = 200, description = "Plugin updated", body = PluginOperationResponse),
        (status = 404, description = "Plugin not found")
    ),
    tag = "plugins",
    security(("bearer_auth" = []))
)]
pub async fn update_plugin_handler(
	Path(name): Path<String>,
	Json(config): Json<ExternalPluginConfig>,
) -> impl IntoResponse {
	if config.name != name {
		return response::error(
			StatusCode::BAD_REQUEST,
			"Path name and body name mismatch.".to_owned(),
		);
	}

	if registry::get_external_plugin(&name).is_none() {
		return response::error(StatusCode::NOT_FOUND, format!("External plugin '{name}' not found."));
	}

	match loader::register_plugin(config).await {
		Ok(_) => response::success(PluginOperationResult { status: "updated".into(), name }),
		Err(e) => response::error(StatusCode::BAD_REQUEST, format!("Failed to update plugin: {e}")),
	}
}

/// Delete external plugin
#[utoipa::path(
    delete,
    path = "/plugins/{name}",
    params(
        ("name" = String, Path, description = "Plugin name")
    ),
    responses(
        (status = 200, description = "Plugin deleted", body = PluginOperationResponse),
        (status = 404, description = "Plugin not found")
    ),
    tag = "plugins",
    security(("bearer_auth" = []))
)]
pub async fn delete_plugin_handler(Path(name): Path<String>) -> impl IntoResponse {
	if registry::get_external_plugin(&name).is_none() {
		return response::error(StatusCode::NOT_FOUND, format!("External plugin '{name}' not found."));
	}

	match loader::delete_plugin(&name).await {
		Ok(_) => response::success(PluginOperationResult { status: "deleted".into(), name }),
		Err(e) => {
			response::error(StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to delete plugin: {e}"))
		}
	}
}
