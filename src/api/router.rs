/* src/api/router.rs */

use crate::{
	api::{
		handlers::applications, handlers::certs, handlers::config, handlers::flow, handlers::nodes,
		handlers::plugins, handlers::ports, handlers::resolvers, handlers::system, middleware::auth,
		middleware::logger, openapi,
	},
	ingress::state::PortState,
};
use axum::{
	Router, middleware,
	routing::{get, post},
};
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

#[cfg(feature = "console")]
pub fn create_router() -> Router<PortState> {
	Router::new()
		.merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", openapi::ApiDoc::openapi()))
		.route("/", get(system::root_handler))
		.route("/health", get(system::health_handler))
		.merge(
			Router::new()
				.route("/status", get(system::status_handler))
				.nest(
					"/ports",
					Router::new()
						.route("/", get(ports::list_ports_handler))
						.route(
							"/{port}",
							get(ports::get_port_handler)
								.post(ports::create_port_handler)
								.delete(ports::delete_port_handler),
						)
						.route(
							"/{port}/{protocol}",
							post(ports::enable_protocol_handler).delete(ports::disable_protocol_handler),
						)
						.route(
							"/{port}/{protocol}/flow",
							get(flow::get_flow_handler)
								.post(flow::post_flow_handler)
								.put(flow::put_flow_handler)
								.delete(flow::delete_flow_handler),
						)
						.route(
							"/{port}/{protocol}/flow/validate",
							post(flow::validate_flow_handler),
						),
				)
				.nest(
					"/plugins",
					Router::new()
						.route("/", get(plugins::list_plugins_handler))
						.route(
							"/{name}",
							get(plugins::get_plugin_handler)
								.post(plugins::create_plugin_handler)
								.put(plugins::update_plugin_handler)
								.delete(plugins::delete_plugin_handler),
						),
				)
				.nest(
					"/nodes",
					Router::new()
						.route(
							"/",
							get(nodes::list_nodes_handler).post(nodes::create_node_handler),
						)
						.route(
							"/{name}",
							get(nodes::get_node_handler)
								.put(nodes::update_node_handler)
								.delete(nodes::delete_node_handler),
						),
				)
				.nest(
					"/certs",
					Router::new()
						.route("/", get(certs::list_certs_handler))
						.route(
							"/{id}",
							get(certs::get_cert_handler)
								.post(certs::upload_cert_handler)
								.delete(certs::delete_cert_handler),
						),
				)
				.nest(
					"/resolvers",
					Router::new()
						.route("/", get(resolvers::list_resolvers_handler))
						.route(
							"/{protocol}",
							get(resolvers::get_resolver_handler)
								.post(resolvers::post_resolver_handler)
								.put(resolvers::put_resolver_handler)
								.delete(resolvers::delete_resolver_handler),
						),
				)
				.nest(
					"/applications",
					Router::new()
						.route("/", get(applications::list_applications_handler))
						.route(
							"/{protocol}",
							get(applications::get_application_handler)
								.post(applications::post_application_handler)
								.put(applications::put_application_handler)
								.delete(applications::delete_application_handler),
						),
				)
				.nest(
					"/config",
					Router::new()
						.route("/reload", post(config::reload_config_handler))
						.route("/export", get(config::export_config_handler))
						.route("/import", post(config::import_config_handler)),
				)
				.layer(middleware::from_fn(auth::require_access_token)),
		)
		.layer(middleware::from_fn(logger::log_requests))
}
