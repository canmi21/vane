/* src/plugins/core/registry.rs */

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
use dashmap::DashMap;
use live::holder::{Store, UnloadPolicy};
use once_cell::sync::Lazy;
use std::sync::Arc;

static INTERNAL_PLUGIN_REGISTRY: Lazy<DashMap<String, Arc<dyn Plugin>>> = Lazy::new(|| {
	let registry = DashMap::new();
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

	for plugin in plugins {
		registry.insert(plugin.name().to_owned(), plugin);
	}

	registry.insert(
		"internal.transport.proxy.transparent".to_owned(),
		transparent_proxy,
	);

	registry
});

static EXTERNAL_PLUGIN_REGISTRY: Lazy<Store<Arc<dyn Plugin>>> = Lazy::new(Store::new);

/// Stores the health status of external plugins.
/// Key: Plugin Name
/// Value: Result<(), ErrorMessage>
pub static EXTERNAL_PLUGIN_STATUS: Lazy<DashMap<String, Result<(), String>>> =
	Lazy::new(DashMap::new);

/// Tracks the last runtime failure (IO error) of external plugins.
/// Key: Plugin Name
/// Value: Instant of last failure
pub static EXTERNAL_PLUGIN_FAILURES: Lazy<DashMap<String, std::time::Instant>> =
	Lazy::new(DashMap::new);

#[must_use]
pub fn get_plugin(name: &str) -> Option<Arc<dyn Plugin>> {
	get_internal_plugin(name).or_else(|| get_external_plugin(name))
}

pub fn get_internal_plugin(name: &str) -> Option<Arc<dyn Plugin>> {
	INTERNAL_PLUGIN_REGISTRY
		.get(name)
		.map(|entry| entry.value().clone())
}

pub fn get_external_plugin(name: &str) -> Option<Arc<dyn Plugin>> {
	EXTERNAL_PLUGIN_REGISTRY
		.get(name)
		.map(|entry| (*entry).clone())
}

pub fn list_external_plugins() -> Vec<Arc<dyn Plugin>> {
	let snapshot = EXTERNAL_PLUGIN_REGISTRY.snapshot();
	snapshot
		.values()
		.map(|entry| (*entry.value).clone())
		.collect()
}

pub fn list_internal_plugins() -> Vec<Arc<dyn Plugin>> {
	INTERNAL_PLUGIN_REGISTRY
		.iter()
		.map(|entry| entry.value().clone())
		.collect()
}

pub fn load_external_plugins(new_plugins: DashMap<String, Arc<dyn Plugin>>) {
	for entry in new_plugins {
		EXTERNAL_PLUGIN_REGISTRY.insert(
			entry.0,
			entry.1,
			std::path::PathBuf::from("memory"),
			UnloadPolicy::Removable,
		);
	}
}

pub fn clear_external_plugins() {
	// Store doesn't have clear(), we could implement it in atomhold, but for now:
	let keys = EXTERNAL_PLUGIN_REGISTRY
		.snapshot()
		.keys()
		.cloned()
		.collect::<Vec<_>>();
	for key in keys {
		let _ = EXTERNAL_PLUGIN_REGISTRY.remove(&key);
	}
}
