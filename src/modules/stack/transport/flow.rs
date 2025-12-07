/* src/modules/stack/transport/flow.rs */

use crate::modules::{
	kv::{KvStore, plugin_output},
	plugins::{
		model::{ConnectionObject, ProcessingStep},
		registry,
	},
};
use anyhow::{Context, Result, anyhow};
use fancy_log::{LogLevel, log};
use serde_json::Value;

/// Public entry point for executing a flow.
/// Initializes the flow path as empty (root).
pub async fn execute(
	step: &ProcessingStep,
	kv: &mut KvStore,
	conn: ConnectionObject,
) -> Result<()> {
	execute_recursive(step, kv, conn, "".to_string()).await
}

/// Internal recursive executor that maintains the current flow path for isolation.
async fn execute_recursive(
	step: &ProcessingStep,
	kv: &mut KvStore,
	conn: ConnectionObject,
	flow_path: String,
) -> Result<()> {
	let (plugin_name, instance) = step
		.iter()
		.next()
		.ok_or_else(|| anyhow!("Empty processing step encountered"))?;

	let resolved_inputs = resolve_inputs(&instance.input, kv);

	let plugin = registry::get_plugin(plugin_name)
		.ok_or_else(|| anyhow!("Plugin '{}' not found in registry", plugin_name))?;

	log(
		LogLevel::Debug,
		&format!(
			"➜ Executing plugin: {} (Path: '{}')",
			plugin_name, flow_path
		),
	);

	// Middleware execution (Intermediate nodes)
	if let Some(middleware) = plugin.as_middleware() {
		let output = middleware
			.execute(resolved_inputs)
			.await
			.with_context(|| format!("Error executing middleware '{}'", plugin_name))?;

		log(
			LogLevel::Debug,
			&format!(
				"✓ Middleware '{}' returned branch: '{}'",
				plugin_name, output.branch
			),
		);

		// --- ENFORCE NAMESPACE ISOLATION BASED ON FLOW PATH ---
		if let Some(updates) = output.store {
			for (raw_key, value) in updates {
				// Key becomes: plugin.{flow_path}.{sanitized_plugin_name}.{raw_key}
				// This ensures that even if the same plugin is used multiple times in different
				// branches/levels, their outputs are stored in distinct keys.
				let namespaced_key = plugin_output::format_scoped_key(&flow_path, plugin_name, &raw_key);

				log(
					LogLevel::Debug,
					&format!("⚙ KV Update (Isolated): {} = {}", namespaced_key, value),
				);
				kv.insert(namespaced_key, value);
			}
		}

		if let Some(next_step) = instance.output.get(output.branch.as_ref()) {
			// Calculate the path for the next step: {current}.{sanitized_plugin}.{branch}
			let next_flow_path =
				plugin_output::next_path(&flow_path, plugin_name, output.branch.as_ref());

			return Box::pin(execute_recursive(next_step, kv, conn, next_flow_path)).await;
		} else {
			return Err(anyhow!(
				"Flow stalled: Middleware '{}' returned branch '{}', but no matching output path is configured.",
				plugin_name,
				output.branch
			));
		}
	}

	// Terminator execution (Leaf nodes)
	if let Some(terminator) = plugin.as_terminator() {
		terminator
			.execute(resolved_inputs, kv, conn)
			.await
			.with_context(|| format!("Error executing terminator '{}'", plugin_name))?;

		log(
			LogLevel::Debug,
			&format!("✓ Flow terminated successfully by '{}'", plugin_name),
		);
		return Ok(());
	}

	Err(anyhow!(
		"Plugin '{}' is neither a valid Middleware nor a Terminator.",
		plugin_name
	))
}

/// Resolves input parameters by replacing `{{key}}` templates with values from the KvStore.
fn resolve_inputs(
	inputs: &std::collections::HashMap<String, Value>,
	kv: &KvStore,
) -> std::collections::HashMap<String, Value> {
	let mut resolved = inputs.clone();

	for (key, value) in inputs {
		if let Some(s) = value.as_str() {
			if s.starts_with("{{") && s.ends_with("}}") {
				let lookup_key = &s[2..s.len() - 2];
				if let Some(kv_value) = kv.get(lookup_key) {
					resolved.insert(key.clone(), Value::String(kv_value.clone()));
				} else {
					log(
						LogLevel::Warn,
						&format!(
							"⚙ Template resolution failed: Key '{}' not found in KvStore.",
							lookup_key
						),
					);
				}
			}
		}
	}
	resolved
}
