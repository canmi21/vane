/* engine/src/daemon/router.rs */

use crate::{
	common::response,
	daemon::root::root_handler,
	middleware::{self, auth::auth_middleware},
	modules::{self, origins},
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
		.route("/v1/origins", get(origins::list_origins))
		.route("/v1/origins", post(origins::create_origin))
		.route("/v1/origins/{:id}", get(origins::get_origin))
		.route("/v1/origins/{:id}", put(origins::update_origin))
		.route("/v1/origins/{:id}", delete(origins::delete_origin))
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
