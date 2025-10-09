/* engine/src/daemon/router.rs */

use crate::{common::response, daemon::root::root_handler, middleware::cors};
use axum::{Router, http::StatusCode, response::IntoResponse, routing::get};

/// The main function to create and configure all application routes.
pub fn create_router() -> Router {
	Router::new()
		// The root endpoint providing application metadata.
		.route("/", get(root_handler))
		// Fallback handler for any request that doesn't match a route.
		.fallback(not_found_handler)
		// Apply the CORS layer to all routes.
		.layer(cors::create_cors_layer())
}

/// A handler for unmatched routes, returning a 404 response.
async fn not_found_handler() -> impl IntoResponse {
	response::error(StatusCode::NOT_FOUND, "Resource not found.".to_string())
}
