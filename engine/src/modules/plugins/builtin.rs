/* engine/src/modules/plugins/builtin.rs */

use super::manager::{Plugin, PluginSource, PluginsStore};
use once_cell::sync::Lazy;
use std::sync::Arc;
use tokio::sync::RwLock;

// A lazy-initialized, thread-safe, and shared global store for all plugins.
// It is initialized with the internal, hardcoded plugins.
pub static PLUGINS: Lazy<Arc<RwLock<PluginsStore>>> = Lazy::new(|| {
	// Internal plugins are hardcoded here.
	let mut store = PluginsStore::new();
	let ratelimit_plugin = Plugin {
		name: "ratelimit".to_string(),
		version: "v1".to_string(),
		source: PluginSource::Internal,
		config: serde_json::json!({
			"description": "Provides rate limiting capabilities."
		}),
	};
	store.insert(
		(
			ratelimit_plugin.name.clone(),
			ratelimit_plugin.version.clone(),
		),
		ratelimit_plugin,
	);

	Arc::new(RwLock::new(store))
});
