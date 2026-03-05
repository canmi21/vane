// Re-export registry data structures and functions from engine crate
pub use vane_engine::registry::*;

// register_builtin_plugins stays here (references main-crate plugin types)
use crate::engine::interfaces::Plugin;
use crate::plugins::l4::{
	abort::AbortConnectionPlugin,
	proxy::{domain::ProxyDomainPlugin, ip::TransparentProxyPlugin, node::ProxyNodePlugin},
};
use crate::plugins::l7::response::SendResponsePlugin;
use crate::plugins::l7::{
	cgi::CgiPlugin, static_files::StaticPlugin, upstream::FetchUpstreamPlugin,
};
use crate::plugins::protocol::detect::ProtocolDetectPlugin;
use crate::plugins::{
	middleware::{
		matcher::CommonMatchPlugin,
		ratelimit::{KeywordRateLimitMinPlugin, KeywordRateLimitSecPlugin},
	},
	protocol::upgrader::upgrade::UpgradePlugin,
};
use std::sync::Arc;

/// Populate the built-in plugin set. Called once during startup.
pub fn register_builtin_plugins() {
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
		Arc::new(CgiPlugin),
		// Terminators (L7)
		Arc::new(SendResponsePlugin),
	];

	#[cfg(feature = "ratelimit")]
	{
		plugins.push(Arc::new(KeywordRateLimitSecPlugin));
		plugins.push(Arc::new(KeywordRateLimitMinPlugin));
	}

	#[cfg(feature = "cgi")]
	{
		plugins.push(Arc::new(CgiPlugin));
	}

	#[cfg(feature = "static")]
	{
		plugins.push(Arc::new(StaticPlugin));
	}

	register_plugins(plugins);
	register_plugin("internal.transport.proxy.transparent", transparent_proxy);
}
