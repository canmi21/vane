/* src/engine/src/registry.rs */

use crate::engine::interfaces::Plugin;
use dashmap::DashMap;
use live::holder::{Store, UnloadPolicy};
use std::sync::Arc;
use std::sync::LazyLock;

static INTERNAL_PLUGIN_REGISTRY: LazyLock<DashMap<String, Arc<dyn Plugin>>> =
	LazyLock::new(DashMap::new);

/// Register an internal plugin by name. Used by bootstrap to populate the
/// registry after all crate-level types are available.
pub fn register_plugin(name: impl Into<String>, plugin: Arc<dyn Plugin>) {
	INTERNAL_PLUGIN_REGISTRY.insert(name.into(), plugin);
}

/// Batch-register plugins, keying each by `Plugin::name()`.
pub fn register_plugins(plugins: Vec<Arc<dyn Plugin>>) {
	for plugin in plugins {
		INTERNAL_PLUGIN_REGISTRY.insert(plugin.name().to_owned(), plugin);
	}
}

static EXTERNAL_PLUGIN_REGISTRY: LazyLock<Store<Arc<dyn Plugin>>> = LazyLock::new(Store::new);

/// Stores the health status of external plugins.
/// Key: Plugin Name
/// Value: Result<(), ErrorMessage>
pub static EXTERNAL_PLUGIN_STATUS: LazyLock<DashMap<String, Result<(), String>>> =
	LazyLock::new(DashMap::new);

/// Tracks the last runtime failure (IO error) of external plugins.
/// Key: Plugin Name
/// Value: Instant of last failure
pub static EXTERNAL_PLUGIN_FAILURES: LazyLock<DashMap<String, std::time::Instant>> =
	LazyLock::new(DashMap::new);

#[must_use]
pub fn get_plugin(name: &str) -> Option<Arc<dyn Plugin>> {
	get_internal_plugin(name).or_else(|| get_external_plugin(name))
}

pub fn get_internal_plugin(name: &str) -> Option<Arc<dyn Plugin>> {
	INTERNAL_PLUGIN_REGISTRY.get(name).map(|entry| entry.value().clone())
}

pub fn get_external_plugin(name: &str) -> Option<Arc<dyn Plugin>> {
	EXTERNAL_PLUGIN_REGISTRY.get(name).map(|entry| (*entry).clone())
}

pub fn list_external_plugins() -> Vec<Arc<dyn Plugin>> {
	let snapshot = EXTERNAL_PLUGIN_REGISTRY.snapshot();
	snapshot.values().map(|entry| (*entry.value).clone()).collect()
}

pub fn list_internal_plugins() -> Vec<Arc<dyn Plugin>> {
	INTERNAL_PLUGIN_REGISTRY.iter().map(|entry| entry.value().clone()).collect()
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
	let keys = EXTERNAL_PLUGIN_REGISTRY.snapshot().keys().cloned().collect::<Vec<_>>();
	for key in keys {
		let _ = EXTERNAL_PLUGIN_REGISTRY.remove(&key);
	}
}
