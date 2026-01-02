/* src/core/router.rs */

use crate::{
	core::root, middleware::auth, middleware::logger,
	modules::plugins::core::handler as plugins_handler, modules::ports::handler as ports_handler,
	modules::ports::model::PortState,
};
use axum::{
	Router, middleware,
	routing::{get, post},
};

#[cfg(feature = "console")]
pub fn create_router() -> Router<PortState> {
	Router::new()
		.route("/", get(root::root_handler))
		.nest(
			"/ports",
			Router::new()
				.route("/", get(ports_handler::get_ports_handler))
				.route(
					"/{port}",
					get(ports_handler::get_port_status_handler)
						.post(ports_handler::post_port_handler)
						.delete(ports_handler::delete_port_handler),
				)
				.route(
					"/{port}/{protocol}",
					post(ports_handler::post_protocol_handler).delete(ports_handler::delete_protocol_handler),
				)
				.layer(middleware::from_fn(auth::require_access_token)),
		)
		.nest(
			"/plugins",
			Router::new()
				.route("/", get(plugins_handler::list_plugins_handler))
				.route(
					"/{name}",
					post(plugins_handler::create_plugin_handler)
						.put(plugins_handler::update_plugin_handler)
						.delete(plugins_handler::delete_plugin_handler),
				)
				.layer(middleware::from_fn(auth::require_access_token)),
		)
		.layer(middleware::from_fn(logger::log_requests))
}
