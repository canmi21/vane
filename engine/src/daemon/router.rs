/* engine/src/daemon/router.rs */

use crate::{
	common::response,
	daemon::root::root_handler,
	middleware::{self, auth::auth_middleware},
	modules::{
		self,
		cache::manager as cache_manager,
		certs::manager as certs_manager,
		cors::manager as cors_manager,
		domain::entrance as domain_entrance,
		header::manager as header_manager,
		origins::{monitor, origins},
		plugins::builtin as plugins_handler, // Added plugin handler
		ratelimit::manager as ratelimit_manager,
		templates::handler as templates_handler,
		websocket::manager as websocket_manager,
	},
};
use axum::{
	Router,
	http::StatusCode,
	middleware::from_fn,
	response::IntoResponse,
	routing::{delete, get, post, put},
};

/// The main function to create and configure all application routes.
pub fn create_router() -> Router {
	// Define the protected API routes first for clarity.
	// Each route and method is defined on its own line.
	let api_routes = Router::new()
		.route("/v1/instance", get(modules::instance::get_instance_info))
		.route("/v1/plugins", get(plugins_handler::list_plugins))
		.route(
			"/v1/plugins/{:name}/{:version}",
			get(plugins_handler::get_plugin)
				.post(plugins_handler::create_plugin)
				.put(plugins_handler::update_plugin)
				.delete(plugins_handler::delete_plugin),
		)
		.route(
			"/v1/origins",
			get(origins::list_origins).post(origins::create_origin),
		)
		.route(
			"/v1/origins/{:id}",
			get(origins::get_origin)
				.put(origins::update_origin)
				.delete(origins::delete_origin),
		)
		.route("/v1/monitor/origins", get(monitor::get_monitor_status))
		.route(
			"/v1/monitor/origins/period",
			put(monitor::update_check_period),
		)
		.route(
			"/v1/monitor/origins/override",
			put(monitor::set_override_url),
		)
		.route(
			"/v1/monitor/origins/override/{:id}",
			delete(monitor::delete_override_url),
		)
		.route(
			"/v1/monitor/origins/task-status",
			get(monitor::get_task_status),
		)
		.route(
			"/v1/monitor/origins/next-check",
			get(monitor::get_next_check_time),
		)
		.route(
			"/v1/monitor/origins/trigger-check",
			post(monitor::trigger_check_now),
		)
		.route("/v1/domains", get(domain_entrance::list_domains))
		.route(
			"/v1/domains/{:domain}",
			post(domain_entrance::create_domain).delete(domain_entrance::delete_domain),
		)
		.route("/v1/certs", get(certs_manager::list_certs))
		.route(
			"/v1/certs/{:domain}",
			get(certs_manager::get_cert_details)
				.post(certs_manager::upload_cert)
				.delete(certs_manager::delete_cert),
		)
		.route("/v1/templates", get(templates_handler::list_templates))
		.route(
			"/v1/templates/{:name}",
			get(templates_handler::get_template_content)
				.post(templates_handler::create_template)
				.put(templates_handler::update_template)
				.delete(templates_handler::delete_template),
		)
		.route("/v1/cors", get(cors_manager::list_cors_status))
		.route(
			"/v1/cors/{:domain}",
			get(cors_manager::get_cors_config)
				.put(cors_manager::update_cors_config)
				.delete(cors_manager::reset_cors_config),
		)
		.route(
			"/v1/headers/{:domain}",
			get(header_manager::get_header_config)
				.put(header_manager::update_header_config)
				.delete(header_manager::reset_header_config),
		)
		.route(
			"/v1/ratelimit/{:domain}",
			get(ratelimit_manager::get_ratelimit_config)
				.put(ratelimit_manager::update_ratelimit_config)
				.delete(ratelimit_manager::reset_ratelimit_config),
		)
		.route(
			"/v1/websocket/{:domain}",
			get(websocket_manager::get_websocket_config)
				.put(websocket_manager::update_websocket_config)
				.delete(websocket_manager::reset_websocket_config),
		)
		.route(
			"/v1/websocket/{:domain}/paths",
			post(websocket_manager::add_websocket_path).delete(websocket_manager::remove_websocket_path),
		)
		.route(
			"/v1/cache/{:domain}",
			get(cache_manager::get_cache_config)
				.put(cache_manager::update_cache_config)
				.delete(cache_manager::reset_cache_config),
		)
		.route(
			"/v1/cache/{:domain}/rules",
			put(cache_manager::add_or_update_cache_rule).delete(cache_manager::remove_cache_rule),
		)
		.route(
			"/v1/cache/{:domain}/blacklist",
			post(cache_manager::add_blacklist_path).delete(cache_manager::remove_blacklist_path),
		)
		// Apply the authentication middleware to all api_routes.
		.layer(from_fn(auth_middleware));

	// Combine unprotected and protected routes into the final router.
	Router::new()
		// Unprotected root endpoint.
		.route("/", get(root_handler))
		// Merge all the protected API routes.
		.merge(api_routes)
		// Fallback handler for any request that doesn't match a route.
		.fallback(not_found_handler)
		// Apply the global CORS layer.
		.layer(middleware::cors::create_cors_layer())
}

/// A handler for unmatched routes, returning a 404 response.
async fn not_found_handler() -> impl IntoResponse {
	response::error(StatusCode::NOT_FOUND, "Resource not found.".to_string())
}
