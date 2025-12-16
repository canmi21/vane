/* src/modules/stack/protocol/application/flow.rs */

use super::container::Container;
use crate::modules::{
	kv::plugin_output,
	plugins::{
		model::{ConnectionObject, ProcessingStep, TerminatorResult},
		registry,
	},
};
use anyhow::{Context, anyhow};
use fancy_log::{LogLevel, log};
use serde_json::Value;
use std::collections::HashMap;

/// Executes the L7 Middleware Pipeline using the Container.
pub async fn execute_l7(
	step: &ProcessingStep,
	container: &mut Container,
	parent_path: String,
) -> anyhow::Result<TerminatorResult> {
	execute_recursive_l7(step, container, parent_path).await
}

async fn execute_recursive_l7(
	step: &ProcessingStep,
	container: &mut Container,
	flow_path: String,
) -> anyhow::Result<TerminatorResult> {
	let (plugin_name, instance) = step
		.iter()
		.next()
		.ok_or_else(|| anyhow!("Empty processing step encountered"))?;

	// 1. Resolve Inputs (Async & Container Aware)
	let resolved_inputs = resolve_inputs_l7(&instance.input, container)
		.await
		.with_context(|| format!("Input resolution failed for '{}'", plugin_name))?;

	let plugin = registry::get_plugin(plugin_name)
		.ok_or_else(|| anyhow!("Plugin '{}' not found", plugin_name))?;

	log(
		LogLevel::Debug,
		&format!("➜ [L7] Executing: {} ({})", plugin_name, flow_path),
	);

	// 2. Execute Middleware
	if let Some(middleware) = plugin.as_middleware() {
		let output = middleware.execute(resolved_inputs).await?;

		// Merge plugin outputs
		if let Some(updates) = output.store {
			for (k, v) in updates {
				let scoped_key = plugin_output::format_scoped_key(&flow_path, plugin_name, &k);
				container.kv.insert(scoped_key, v);
			}
		}

		// Branching
		if let Some(next_step) = instance.output.get(output.branch.as_ref()) {
			let next_path = plugin_output::next_path(&flow_path, plugin_name, output.branch.as_ref());
			return Box::pin(execute_recursive_l7(next_step, container, next_path)).await;
		} else {
			return Err(anyhow!(
				"Flow stalled at '{}': branch '{}' not handled",
				plugin_name,
				output.branch
			));
		}
	}

	// 3. Execute Terminator
	if let Some(terminator) = plugin.as_terminator() {
		// Use Virtual Connection for L7 internal termination
		let conn_placeholder = ConnectionObject::Virtual("L7_Managed_Context".into());
		return terminator
			.execute(resolved_inputs, &mut container.kv, conn_placeholder)
			.await;
	}

	Err(anyhow!(
		"Plugin '{}' type mismatch: Expected Middleware or Terminator",
		plugin_name
	))
}

/// Resolves input templates, triggering Lazy Buffering for specific Magic Words.
async fn resolve_inputs_l7(
	inputs: &HashMap<String, Value>,
	container: &mut Container,
) -> anyhow::Result<HashMap<String, Value>> {
	let mut resolved = inputs.clone();

	for (key, value) in inputs {
		if let Some(s) = value.as_str() {
			if s.starts_with("{{") && s.ends_with("}}") {
				let lookup_key = &s[2..s.len() - 2];

				// --- Magic Word: Payload Access ---
				// We support both req.* and res.* variants.
				// The semantic meaning depends on which phase of the pipeline we are in,
				// but physically they both map to the current `container.payload`.
				if matches!(
					lookup_key,
					"req.body" | "req.body_hex" | "res.body" | "res.body_hex"
				) {
					// TRIGGER: Lazy Buffer
					let bytes = container.force_buffer().await?;

					let is_hex = lookup_key.ends_with("_hex");
					let val_str = if is_hex {
						hex::encode(bytes)
					} else {
						String::from_utf8_lossy(bytes).to_string()
					};

					resolved.insert(key.clone(), Value::String(val_str));
					continue;
				}

				// --- Standard Metadata Lookup ---
				if let Some(kv_val) = container.kv.get(lookup_key) {
					resolved.insert(key.clone(), Value::String(kv_val.clone()));
				} else {
					log(
						LogLevel::Warn,
						&format!("⚠ Template '{}' not found in Container KV.", lookup_key),
					);
				}
			}
		}
	}
	Ok(resolved)
}
