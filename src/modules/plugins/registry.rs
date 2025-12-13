/* src/modules/plugins/registry.rs */

use super::{
	common::{
		matcher::CommonMatchPlugin,
		ratelimit::{KeywordRateLimitMinPlugin, KeywordRateLimitSecPlugin},
	},
	model::Plugin,
	protocol::detect::ProtocolDetectPlugin,
	terminator::{
		transport::{
			abort::AbortConnectionPlugin,
			proxy::{domain::ProxyDomainPlugin, ip::TransparentProxyPlugin, node::ProxyNodePlugin},
		},
		upgrader::upgrade::UpgradePlugin,
	},
};
use arc_swap::ArcSwap;
use dashmap::DashMap;
use once_cell::sync::Lazy;
use std::sync::Arc;

static INTERNAL_PLUGIN_REGISTRY: Lazy<DashMap<String, Arc<dyn Plugin>>> = Lazy::new(|| {
	let registry = DashMap::new();
	let transparent_proxy = Arc::new(TransparentProxyPlugin);

	let plugins: Vec<Arc<dyn Plugin>> = vec![
		// Core Logic
		Arc::new(ProtocolDetectPlugin),
		// Universal Matcher
		Arc::new(CommonMatchPlugin),
		// Terminators
		Arc::new(AbortConnectionPlugin),
		transparent_proxy.clone(),
		Arc::new(ProxyNodePlugin),
		Arc::new(ProxyDomainPlugin),
		Arc::new(UpgradePlugin),
		// Ratelimits
		Arc::new(KeywordRateLimitSecPlugin),
		Arc::new(KeywordRateLimitMinPlugin),
	];

	for plugin in plugins {
		registry.insert(plugin.name().to_string(), plugin);
	}

	registry.insert(
		"internal.transport.proxy.transparent".to_string(),
		transparent_proxy,
	);

	registry
});

static EXTERNAL_PLUGIN_REGISTRY: Lazy<ArcSwap<DashMap<String, Arc<dyn Plugin>>>> =
	Lazy::new(|| ArcSwap::new(Arc::new(DashMap::new())));

pub fn get_plugin(name: &str) -> Option<Arc<dyn Plugin>> {
	get_internal_plugin(name).or_else(|| get_external_plugin(name))
}

pub fn get_internal_plugin(name: &str) -> Option<Arc<dyn Plugin>> {
	INTERNAL_PLUGIN_REGISTRY
		.get(name)
		.map(|entry| entry.value().clone())
}

pub fn get_external_plugin(name: &str) -> Option<Arc<dyn Plugin>> {
	let external_plugins = EXTERNAL_PLUGIN_REGISTRY.load();
	external_plugins
		.get(name)
		.map(|plugin| plugin.value().clone())
}

pub fn list_external_plugins() -> Vec<Arc<dyn Plugin>> {
	let external_plugins = EXTERNAL_PLUGIN_REGISTRY.load();
	external_plugins
		.iter()
		.map(|entry| entry.value().clone())
		.collect()
}

pub fn load_external_plugins(new_plugins: DashMap<String, Arc<dyn Plugin>>) {
	EXTERNAL_PLUGIN_REGISTRY.store(Arc::new(new_plugins));
}

pub fn clear_external_plugins() {
	EXTERNAL_PLUGIN_REGISTRY.store(Arc::new(DashMap::new()));
}
