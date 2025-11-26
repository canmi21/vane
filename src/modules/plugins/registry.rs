/* src/modules/plugins/registry.rs */

use super::{
	model::Plugin,
	protocol::detect::ProtocolDetectPlugin,
	terminator::transport::{
		abort_connection::AbortConnectionPlugin, transparent_proxy::TransparentProxyPlugin,
	},
};
use dashmap::DashMap;
use once_cell::sync::Lazy;
use std::sync::Arc;

/// A global, thread-safe registry for all known plugins.
static PLUGIN_REGISTRY: Lazy<DashMap<String, Arc<dyn Plugin>>> = Lazy::new(|| {
	let registry = DashMap::new();

	let plugins: Vec<Arc<dyn Plugin>> = vec![
		Arc::new(ProtocolDetectPlugin),
		Arc::new(AbortConnectionPlugin),
		Arc::new(TransparentProxyPlugin),
	];

	for plugin in plugins {
		registry.insert(plugin.name().to_string(), plugin);
	}

	registry
});

/// Finds a plugin by its name in the global registry.
pub fn get_plugin(name: &str) -> Option<Arc<dyn Plugin>> {
	PLUGIN_REGISTRY.get(name).map(|entry| entry.value().clone())
}
