/* src/core/router.rs */

use crate::{
	core::{response, root::root_handler},
	middleware::logger,
	modules::ports::{handler as ports_handler, model::PortState},
};
use axum::{
	Router,
	http::StatusCode,
	middleware,
	response::IntoResponse,
	routing::{get, post},
};

// The function signature now honestly declares that it returns a router
// whose handlers require a state of type `PortState`.
pub fn create_router() -> Router<PortState> {
	Router::new()
		.route("/", get(root_handler))
		.route("/ports", get(ports_handler::get_ports_handler))
		.route(
			"/ports/{:port}",
			post(ports_handler::post_port_handler)
				.delete(ports_handler::delete_port_handler)
				.get(ports_handler::get_port_status_handler),
		)
		.route(
			"/ports/{:port}/{:protocol}",
			post(ports_handler::post_protocol_handler).delete(ports_handler::delete_protocol_handler),
		)
		.layer(middleware::from_fn(logger::log_requests))
		.fallback(not_found_handler)
}

async fn not_found_handler() -> impl IntoResponse {
	response::error(StatusCode::NOT_FOUND, "Resource not found.".to_string())
}
