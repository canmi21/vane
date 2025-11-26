/* src/modules/plugins/registry.rs */

use super::{
	model::Plugin,
	protocol::detect::ProtocolDetectPlugin,
	terminator::transport::{
		abort_connection::AbortConnectionPlugin, transparent_proxy::TransparentProxyPlugin,
	},
};
use arc_swap::ArcSwap;
use dashmap::DashMap;
use once_cell::sync::Lazy;
use std::sync::Arc;

/// A static, compile-time registry for all built-in plugins.
/// This registry is immutable at runtime, ensuring core functionality is secure.
static INTERNAL_PLUGIN_REGISTRY: Lazy<DashMap<String, Arc<dyn Plugin>>> = Lazy::new(|| {
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

/// An atomically swappable registry for dynamically loaded external plugins.
/// This allows for safe, zero-downtime hot-reloading of external plugins.
static EXTERNAL_PLUGIN_REGISTRY: Lazy<ArcSwap<DashMap<String, Arc<dyn Plugin>>>> =
	Lazy::new(|| ArcSwap::new(Arc::new(DashMap::new())));

/// Finds a plugin by its name, searching internal plugins first, then external ones.
///
/// This is the primary lookup function. It prioritizes built-in plugins to ensure
/// that core functionality cannot be overridden by external plugins.
pub fn get_plugin(name: &str) -> Option<Arc<dyn Plugin>> {
	get_internal_plugin(name).or_else(|| get_external_plugin(name))
}

/// Finds a plugin by name exclusively within the internal (built-in) registry.
pub fn get_internal_plugin(name: &str) -> Option<Arc<dyn Plugin>> {
	INTERNAL_PLUGIN_REGISTRY
		.get(name)
		.map(|entry| entry.value().clone())
}

/// Finds a plugin by name exclusively within the external (dynamic) registry.
pub fn get_external_plugin(name: &str) -> Option<Arc<dyn Plugin>> {
	let external_plugins = EXTERNAL_PLUGIN_REGISTRY.load();
	external_plugins
		.get(name)
		.map(|plugin| plugin.value().clone())
}

/// Atomically replaces the entire set of external plugins.
/// This is the entry point for a future hot-reloading mechanism.
pub fn load_external_plugins(new_plugins: DashMap<String, Arc<dyn Plugin>>) {
	EXTERNAL_PLUGIN_REGISTRY.store(Arc::new(new_plugins));
}

/// Atomically clears all external plugins, restoring the system to a
/// built-in-only state.
pub fn clear_external_plugins() {
	EXTERNAL_PLUGIN_REGISTRY.store(Arc::new(DashMap::new()));
}
