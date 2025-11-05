/* engine/src/modules/plugins/builtin.rs */

use super::manager::{
	Author, OutputResults, ParamDefinition, Plugin, PluginInterface, PluginsStore, VariableDefinition,
};
use once_cell::sync::Lazy;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;

// A lazy-initialized, thread-safe, and shared global store for all plugins.
// It is initialized with the internal, hardcoded plugins.
pub static PLUGINS: Lazy<Arc<RwLock<PluginsStore>>> = Lazy::new(|| {
	let mut store = PluginsStore::new();

	// --- Define the author once for reuse ---
	let default_author = Author {
		name: "Canmi".to_string(),
		url: "https://github.com/canmi21".to_string(),
	};

	// --- 1. Define the 'ratelimit' internal plugin ---
	let ratelimit_plugin = Plugin {
		name: "ratelimit".to_string(),
		version: "v1".to_string(),
		description: "Provides keyword-based rate limiting capabilities.".to_string(),
		authors: vec![default_author.clone()],
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
			..Default::default()
		},
	};
	store.insert(
		(
			ratelimit_plugin.name.clone(),
			ratelimit_plugin.version.clone(),
		),
		ratelimit_plugin,
	);

	// --- 2. Define the 'origins' internal plugin ---
	let origins_plugin = Plugin {
		name: "origins".to_string(),
		version: "v1".to_string(),
		description: "Forwards the request to a specified origin server.".to_string(),
		authors: vec![default_author.clone()],
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
			..Default::default()
		},
	};
	store.insert(
		(origins_plugin.name.clone(), origins_plugin.version.clone()),
		origins_plugin,
	);

	// --- 3. Define the 'error-page' terminal plugin ---
	let error_page_plugin = Plugin {
		name: "error-page".to_string(),
		version: "v1".to_string(),
		description: "Returns a customizable error page to the client.".to_string(),
		authors: vec![default_author.clone()],
		interface: PluginInterface {
			r#type: "internal".to_string(),
		},
		input_params: HashMap::from([
			(
				"status_code".to_string(),
				ParamDefinition {
					r#type: "number".to_string(),
				},
			),
			(
				"status_description".to_string(),
				ParamDefinition {
					r#type: "string".to_string(),
				},
			),
			(
				"reason".to_string(),
				ParamDefinition {
					r#type: "string".to_string(),
				},
			),
			(
				"request_id".to_string(),
				ParamDefinition {
					r#type: "string".to_string(),
				},
			),
			(
				"timestamp".to_string(),
				ParamDefinition {
					r#type: "string".to_string(),
				},
			),
			(
				"version".to_string(),
				ParamDefinition {
					r#type: "string".to_string(),
				},
			),
			(
				"request_ip".to_string(),
				ParamDefinition {
					r#type: "string".to_string(),
				},
			),
			(
				"visitor_tip".to_string(),
				ParamDefinition {
					r#type: "string".to_string(),
				},
			),
			(
				"admin_guide".to_string(),
				ParamDefinition {
					r#type: "string".to_string(),
				},
			),
		]),
		output_results: OutputResults {
			r#return: true,
			..Default::default()
		},
	};
	store.insert(
		(
			error_page_plugin.name.clone(),
			error_page_plugin.version.clone(),
		),
		error_page_plugin,
	);

	// --- 4. Define the 'return-response' terminal plugin ---
	let return_response_plugin = Plugin {
		name: "return-response".to_string(),
		version: "v1".to_string(),
		description: "Returns a final HTTP response to the client.".to_string(),
		authors: vec![default_author.clone()],
		interface: PluginInterface {
			r#type: "internal".to_string(),
		},
		input_params: HashMap::from([
			(
				"status_code".to_string(),
				ParamDefinition {
					r#type: "number".to_string(),
				},
			),
			(
				"header".to_string(),
				ParamDefinition {
					r#type: "string".to_string(),
				},
			),
			(
				"body".to_string(),
				ParamDefinition {
					r#type: "string".to_string(),
				},
			),
		]),
		output_results: OutputResults {
			r#return: true,
			..Default::default()
		},
	};
	store.insert(
		(
			return_response_plugin.name.clone(),
			return_response_plugin.version.clone(),
		),
		return_response_plugin,
	);

	Arc::new(RwLock::new(store))
});
