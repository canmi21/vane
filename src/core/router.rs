/* src/core/router.rs */

use crate::{core::response, core::root::root_handler};
use axum::{Router, http::StatusCode, response::IntoResponse, routing::get};

pub fn create_router() -> Router {
	Router::new()
		.route("/", get(root_handler))
		.fallback(not_found_handler)
}

async fn not_found_handler() -> impl IntoResponse {
	response::error(StatusCode::NOT_FOUND, "Resource not found.".to_string())
}
