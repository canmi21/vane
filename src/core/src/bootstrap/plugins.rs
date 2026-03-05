/* src/core/src/bootstrap/plugins.rs */

use std::sync::Arc;
use vane_engine::engine::interfaces::Plugin;
use vane_engine::registry::{register_plugin, register_plugins};

/// Populate the built-in plugin set. Called once during startup.
pub fn register_builtin_plugins() {
	use vane_app::plugins::upstream::FetchUpstreamPlugin;
	use vane_app::upgrader::upgrade::UpgradePlugin;
	use vane_extra::l4::abort::AbortConnectionPlugin;
	use vane_extra::l4::proxy::domain::ProxyDomainPlugin;
	use vane_extra::l4::proxy::ip::TransparentProxyPlugin;
	use vane_extra::l4::proxy::node::ProxyNodePlugin;
	use vane_extra::middleware::matcher::CommonMatchPlugin;
	use vane_transport::protocol::detect::ProtocolDetectPlugin;

	let transparent_proxy = Arc::new(TransparentProxyPlugin);

	let mut plugins: Vec<Arc<dyn Plugin>> = vec![
		// Core Logic
		Arc::new(ProtocolDetectPlugin),
		// Universal Matcher
		Arc::new(CommonMatchPlugin),
		// Terminators (L4/L4+)
		Arc::new(AbortConnectionPlugin),
		transparent_proxy.clone(),
		Arc::new(ProxyNodePlugin),
		Arc::new(ProxyDomainPlugin),
		Arc::new(UpgradePlugin),
		// Drivers (L7)
		#[cfg(any(feature = "h2upstream", feature = "h3upstream"))]
		Arc::new(FetchUpstreamPlugin),
		// Terminators (L7)
		Arc::new(vane_app::plugins::response::SendResponsePlugin),
	];

	#[cfg(feature = "ratelimit")]
	{
		use vane_extra::middleware::ratelimit::{KeywordRateLimitMinPlugin, KeywordRateLimitSecPlugin};
		plugins.push(Arc::new(KeywordRateLimitSecPlugin));
		plugins.push(Arc::new(KeywordRateLimitMinPlugin));
	}

	#[cfg(feature = "cgi")]
	{
		use vane_app::plugins::cgi::CgiPlugin;
		plugins.push(Arc::new(CgiPlugin));
	}

	#[cfg(feature = "static")]
	{
		use vane_app::plugins::static_files::StaticPlugin;
		plugins.push(Arc::new(StaticPlugin));
	}

	register_plugins(plugins);
	register_plugin("internal.transport.proxy.transparent", transparent_proxy);
}
