/* src/modules/stack/protocol/carrier/flow.rs */

use crate::modules::{
	kv::{KvStore, plugin_output},
	plugins::{
		model::{ConnectionObject, ProcessingStep, TerminatorResult},
		registry,
	},
};
use anyhow::{Context, Result, anyhow};
use fancy_log::{LogLevel, log};
use serde_json::Value;

pub async fn execute(
	step: &ProcessingStep,
	kv: &mut KvStore,
	conn: ConnectionObject,
	parent_path: String,
) -> Result<TerminatorResult> {
	kv.insert("conn.layer".to_string(), "l4p".to_string());
	execute_recursive(step, kv, conn, parent_path).await
}

async fn execute_recursive(
	step: &ProcessingStep,
	kv: &mut KvStore,
	conn: ConnectionObject,
	flow_path: String,
) -> Result<TerminatorResult> {
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

		if let Some(updates) = output.store {
			for (raw_key, value) in updates {
				let namespaced_key = plugin_output::format_scoped_key(&flow_path, plugin_name, &raw_key);
				log(
					LogLevel::Debug,
					&format!("⚙ KV Update: {} = {}", namespaced_key, value),
				);
				kv.insert(namespaced_key, value);
			}
		}

		if let Some(next_step) = instance.output.get(output.branch.as_ref()) {
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

	if let Some(terminator) = plugin.as_terminator() {
		let result = terminator
			.execute(resolved_inputs, kv, conn)
			.await
			.with_context(|| format!("Error executing terminator '{}'", plugin_name))?;

		match result {
			TerminatorResult::Finished => {
				log(
					LogLevel::Debug,
					&format!("✓ Flow terminated successfully by '{}'", plugin_name),
				);
				Ok(TerminatorResult::Finished)
			}
			TerminatorResult::Upgrade { protocol, conn, .. } => {
				log(
					LogLevel::Info,
					&format!(
						"➜ Flow upgrade requested by '{}' -> Protocol: {}",
						plugin_name, protocol
					),
				);
				// FIXED: Use actual protocol name
				let upgrade_path = plugin_output::next_path(&flow_path, plugin_name, &protocol);
				Ok(TerminatorResult::Upgrade {
					protocol,
					conn,
					parent_path: upgrade_path,
				})
			}
		}
	} else {
		Err(anyhow!(
			"Plugin '{}' is neither a valid Middleware nor a Terminator.",
			plugin_name
		))
	}
}

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
