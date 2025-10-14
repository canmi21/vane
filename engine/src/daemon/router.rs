/* engine/src/daemon/router.rs */

use crate::{
	common::response,
	daemon::root::root_handler,
	middleware::{self, auth::auth_middleware},
	modules,
};
use axum::{Router, http::StatusCode, middleware::from_fn, response::IntoResponse, routing::get};

/// The main function to create and configure all application routes.
pub fn create_router() -> Router {
	Router::new()
		.route("/", get(root_handler))
		.route("/v1/instance", get(modules::instance::get_instance_info))
		.fallback(not_found_handler)
		.layer(from_fn(auth_middleware))
		.layer(middleware::cors::create_cors_layer())
}

/// A handler for unmatched routes, returning a 404 response.
async fn not_found_handler() -> impl IntoResponse {
	response::error(StatusCode::NOT_FOUND, "Resource not found.".to_string())
}
