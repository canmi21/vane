/* engine/src/modules/plugins/builtin.rs */

use super::manager::{
	OutputResults, ParamDefinition, Plugin, PluginInterface, PluginsStore, VariableDefinition,
};
use once_cell::sync::Lazy;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;

// A lazy-initialized, thread-safe, and shared global store for all plugins.
// It is initialized with the internal, hardcoded plugins.
pub static PLUGINS: Lazy<Arc<RwLock<PluginsStore>>> = Lazy::new(|| {
	let mut store = PluginsStore::new();

	// --- Define the 'ratelimit' internal plugin ---
	let ratelimit_plugin = Plugin {
		name: "ratelimit".to_string(),
		version: "v1".to_string(),
		description: "Provides keyword-based rate limiting capabilities.".to_string(),
		author: "Canmi".to_string(),
		url: "https://github.com/canmi21".to_string(),
		interface: PluginInterface {
			r#type: "internal".to_string(),
		},
		input_params: HashMap::from([
			(
				"id".to_string(),
				ParamDefinition {
					r#type: "string".to_string(),
				},
			),
			(
				"host".to_string(),
				ParamDefinition {
					r#type: "string".to_string(),
				},
			),
			(
				"req/s".to_string(),
				ParamDefinition {
					r#type: "number".to_string(),
				},
			),
		]),
		output_results: OutputResults {
			tree: vec!["accept".to_string(), "drop".to_string()],
			variables: HashMap::<String, VariableDefinition>::new(),
		},
	};

	store.insert(
		(
			ratelimit_plugin.name.clone(),
			ratelimit_plugin.version.clone(),
		),
		ratelimit_plugin,
	);

	// --- Define the 'origins' internal plugin ---
	let origins_plugin = Plugin {
		name: "origins".to_string(),
		version: "v1".to_string(),
		description: "Forwards the request to a specified origin server.".to_string(),
		author: "Canmi".to_string(),
		url: "https://github.com/canmi21".to_string(),
		interface: PluginInterface {
			r#type: "internal".to_string(),
		},
		input_params: HashMap::from([
			(
				"transport".to_string(),
				ParamDefinition {
					r#type: "string".to_string(),
				},
			),
			(
				"scheme".to_string(),
				ParamDefinition {
					r#type: "string".to_string(),
				},
			),
			(
				"authority".to_string(),
				ParamDefinition {
					r#type: "string".to_string(),
				},
			),
			(
				"path".to_string(),
				ParamDefinition {
					r#type: "string".to_string(),
				},
			),
			(
				"timeout s".to_string(),
				ParamDefinition {
					r#type: "number".to_string(),
				},
			),
			(
				"tls verify".to_string(),
				ParamDefinition {
					r#type: "boolean".to_string(),
				},
			),
		]),
		output_results: OutputResults {
			tree: vec!["up".to_string(), "down".to_string()],
			variables: HashMap::from([
				(
					"status".to_string(),
					VariableDefinition {
						r#type: "number".to_string(),
					},
				),
				(
					"headers".to_string(),
					VariableDefinition {
						r#type: "string".to_string(),
					},
				),
				(
					"body".to_string(),
					VariableDefinition {
						r#type: "string".to_string(),
					},
				),
			]),
		},
	};

	store.insert(
		(origins_plugin.name.clone(), origins_plugin.version.clone()),
		origins_plugin,
	);

	Arc::new(RwLock::new(store))
});
