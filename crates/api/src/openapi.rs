/* src/api/openapi.rs */

use crate::handlers::{
	applications, certs, config, flow, nodes, plugins, ports, resolvers, system,
};
use crate::schemas::{
	applications as app_schemas, certs as cert_schemas, config as config_schemas,
	flow as flow_schemas, nodes as node_schemas, plugins as plugin_schemas, ports as port_schemas,
	resolvers as res_schemas, system as system_schemas,
};
use utoipa::OpenApi;
use vane_engine::config::ApplicationConfig;
use vane_engine::config::ResolverConfig;
use vane_engine::engine::interfaces::{
	ExternalParamDef, ExternalPluginConfig, ExternalPluginDriver, PluginInstance, PluginRole,
};
use vane_primitives::service_discovery::model::{IpConfig, IpType, Node};

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Vane API",
        version = env!("CARGO_PKG_VERSION"),
        description = "Vane Reverse Proxy Management API",
        license(name = "MIT", url = "https://opensource.org/licenses/MIT"),
        contact(name = "Canmi", email = "t@canmi.icu")
    ),
    paths(
        system::root_handler,
        system::health_handler,
        system::status_handler,
        ports::list_ports_handler,
        ports::get_port_handler,
        ports::create_port_handler,
        ports::delete_port_handler,
        ports::enable_protocol_handler,
        ports::disable_protocol_handler,
        flow::get_flow_handler,
        flow::post_flow_handler,
        flow::put_flow_handler,
        flow::delete_flow_handler,
        flow::validate_flow_handler,
        plugins::list_plugins_handler,
        plugins::get_plugin_handler,
        plugins::create_plugin_handler,
        plugins::update_plugin_handler,
        plugins::delete_plugin_handler,
        nodes::list_nodes_handler,
        nodes::get_node_handler,
        nodes::create_node_handler,
        nodes::update_node_handler,
        nodes::delete_node_handler,
        certs::list_certs_handler,
        certs::get_cert_handler,
        certs::upload_cert_handler,
        certs::delete_cert_handler,
        resolvers::list_resolvers_handler,
        resolvers::get_resolver_handler,
        resolvers::post_resolver_handler,
        resolvers::put_resolver_handler,
        resolvers::delete_resolver_handler,
        applications::list_applications_handler,
        applications::get_application_handler,
        applications::post_application_handler,
        applications::put_application_handler,
        applications::delete_application_handler,
        config::reload_config_handler,
        config::export_config_handler,
        config::import_config_handler,
    ),
    components(
        schemas(
            // Explicit Response Schemas
            system_schemas::SystemInfoResponse,
            system_schemas::HealthStatusResponse,
            system_schemas::SystemStatusResponse,
            port_schemas::PortListResponse,
            port_schemas::PortDetailResponse,
            port_schemas::PortCreatedResponse,
            flow_schemas::FlowConfigResponse,
            flow_schemas::FlowConfigWrittenResponse,
            flow_schemas::ValidationResultResponse,
            plugin_schemas::PluginListResponse,
            plugin_schemas::PluginDetailResponse,
            plugin_schemas::PluginOperationResponse,
            node_schemas::NodeListResponse,
            node_schemas::NodeDetailResponse,
            node_schemas::NodeOperationResponse,
            cert_schemas::CertListResponse,
            cert_schemas::CertDetailResponse,
            cert_schemas::CertOperationResponse,
            res_schemas::ResolverListResponse,
            res_schemas::ResolverDetailResponse,
            app_schemas::ApplicationListResponse,
            app_schemas::ApplicationDetailResponse,
            config_schemas::ReloadResponse,
            config_schemas::ImportResponse,

            // System Schemas
            system_schemas::SystemInfo,
            system_schemas::PackageInfo,
            system_schemas::BuildInfo,
            system_schemas::RuntimeInfo,
            system_schemas::HealthStatus,
            system_schemas::SystemStatusDetails,

            // Port Schemas
            port_schemas::PortInfo,
            port_schemas::PortDetail,
            port_schemas::ProtocolStatus,
            port_schemas::PortCreated,

            // Flow Schemas
            flow_schemas::FlowConfig,
            flow_schemas::FlowConfigData,
            flow_schemas::FlowConfigWritten,
            flow_schemas::ValidationResult,

            // Plugin Schemas
            plugin_schemas::PluginSummary,
            plugin_schemas::PluginList,
            plugin_schemas::PluginDetail,
            plugin_schemas::ParamDefResponse,
            plugin_schemas::PluginOperationResult,
            ExternalPluginConfig,
            ExternalPluginDriver,
            PluginRole,
            ExternalParamDef,
            PluginInstance,

            // Node Schemas
            Node,
            IpConfig,
            IpType,
            node_schemas::NodeListData,
            node_schemas::NodeOperationResult,

            // Cert Schemas
            cert_schemas::CertSummary,
            cert_schemas::CertDetail,
            cert_schemas::CertUploadRequest,
            cert_schemas::CertOperationResult,

            // Resolver Schemas
            res_schemas::ResolverSummary,
            res_schemas::ResolverListData,
            res_schemas::ResolverDetail,
            ResolverConfig,

            // Application Schemas
            app_schemas::ApplicationSummary,
            app_schemas::ApplicationListData,
            app_schemas::ApplicationDetail,
            ApplicationConfig,

            // Config Schemas
            config_schemas::ReloadRequest,
            config_schemas::ReloadResult,
            config_schemas::ImportResult,
        )
    ),
    tags(
        (name = "system", description = "System information and status"),
        (name = "ports", description = "Port management"),
        (name = "flow", description = "Flow configuration management"),
        (name = "plugins", description = "Plugin management"),
        (name = "nodes", description = "Service discovery and backend nodes"),
        (name = "certs", description = "SSL/TLS certificates"),
        (name = "resolvers", description = "L4+ protocol resolvers"),
        (name = "applications", description = "L7 application protocols"),
        (name = "config", description = "Configuration operations")
    ),
    modifiers(&SecurityAddon)
)]
pub struct ApiDoc;

use utoipa::Modify;
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};

pub struct SecurityAddon;

impl Modify for SecurityAddon {
	fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
		let components = openapi.components.as_mut().unwrap();
		components.add_security_scheme(
			"bearer_auth",
			SecurityScheme::Http(
				HttpBuilder::new()
					.scheme(HttpAuthScheme::Bearer)
					.bearer_format("JWT")
					.build(),
			),
		);
	}
}
