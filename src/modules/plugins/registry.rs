/* src/modules/plugins/registry.rs */

use super::{
	common::ratelimit::{KeywordRateLimitMinPlugin, KeywordRateLimitSecPlugin},
	model::Plugin,
	protocol::detect::ProtocolDetectPlugin,
	terminator::transport::{
		abort_connection::AbortConnectionPlugin, proxy_domain::ProxyDomainPlugin,
		proxy_node::ProxyNodePlugin, transparent_proxy::TransparentProxyPlugin, upgrade::UpgradePlugin,
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

	// Create shared instances for plugins that require aliases
	let transparent_proxy = Arc::new(TransparentProxyPlugin);

	let plugins: Vec<Arc<dyn Plugin>> = vec![
		Arc::new(ProtocolDetectPlugin),
		Arc::new(AbortConnectionPlugin),
		// Register the TransparentProxyPlugin (Name: internal.transport.proxy)
		transparent_proxy.clone(),
		Arc::new(ProxyNodePlugin),
		Arc::new(ProxyDomainPlugin),
		Arc::new(UpgradePlugin),
		Arc::new(KeywordRateLimitSecPlugin),
		Arc::new(KeywordRateLimitMinPlugin),
	];

	// Standard Registration
	for plugin in plugins {
		registry.insert(plugin.name().to_string(), plugin);
	}

	// Alias Registration: Maintain backward compatibility for "internal.transport.proxy.transparent"
	// This ensures both keys point to the exact same plugin instance.
	registry.insert(
		"internal.transport.proxy.transparent".to_string(),
		transparent_proxy,
	);

	registry
});

/// An atomically swappable registry for dynamically loaded external plugins.
/// This allows for safe, zero-downtime hot-reloading of external plugins.
static EXTERNAL_PLUGIN_REGISTRY: Lazy<ArcSwap<DashMap<String, Arc<dyn Plugin>>>> =
	Lazy::new(|| ArcSwap::new(Arc::new(DashMap::new())));

/// Finds a plugin by its name, searching internal plugins first, then external ones.
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

/// Returns a list of all currently registered external plugins.
pub fn list_external_plugins() -> Vec<Arc<dyn Plugin>> {
	let external_plugins = EXTERNAL_PLUGIN_REGISTRY.load();
	external_plugins
		.iter()
		.map(|entry| entry.value().clone())
		.collect()
}

/// Atomically replaces the entire set of external plugins.
/// This is the entry point for bulk loading or hot-swapping.
pub fn load_external_plugins(new_plugins: DashMap<String, Arc<dyn Plugin>>) {
	EXTERNAL_PLUGIN_REGISTRY.store(Arc::new(new_plugins));
}

/// Atomically clears all external plugins.
pub fn clear_external_plugins() {
	EXTERNAL_PLUGIN_REGISTRY.store(Arc::new(DashMap::new()));
}
