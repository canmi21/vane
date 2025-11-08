/* src/core/router.rs */

use crate::{
	core::{response, root::root_handler},
	middleware::logger,
	modules::ports::handler as ports_handler,
};
use axum::{
	Router,
	http::StatusCode,
	middleware,
	response::IntoResponse,
	routing::{get, post},
};

pub fn create_router() -> Router {
	Router::new()
		// Define all application routes together.
		.route("/", get(root_handler))
		.route("/ports", get(ports_handler::get_ports_handler))
		.route(
			"/ports/{:port}",
			post(ports_handler::post_port_handler).delete(ports_handler::delete_port_handler),
		)
		// Apply the single, smarter logger middleware to ALL routes defined above.
		.layer(middleware::from_fn(logger::log_requests))
		// The fallback handler is applied after the routes and layers.
		.fallback(not_found_handler)
}

async fn not_found_handler() -> impl IntoResponse {
	response::error(StatusCode::NOT_FOUND, "Resource not found.".to_string())
}
