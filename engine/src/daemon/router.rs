/* engine/src/daemon/router.rs */

use crate::{
	common::response,
	daemon::root::root_handler,
	middleware::{self, auth::auth_middleware},
	modules::{
		self,
		certs::manager as certs_manager,
		domain::entrance as domain_entrance,
		origins::{monitor, origins},
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
