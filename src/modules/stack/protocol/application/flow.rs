/* src/modules/stack/protocol/application/flow.rs */

use super::container::Container;
use crate::modules::{
	kv::plugin_output,
	plugins::{
		model::{ConnectionObject, MiddlewareOutput, ProcessingStep, TerminatorResult},
		registry,
	},
};
use anyhow::{Context, anyhow};
use fancy_log::{LogLevel, log};
use http::HeaderMap;
use serde_json::Value;
use std::collections::HashMap;

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

	// 1. Resolve Inputs (The Smart Template System)
	let resolved_inputs = resolve_inputs_l7(&instance.input, container)
		.await
		.with_context(|| format!("Input resolution failed for '{}'", plugin_name))?;

	let plugin = registry::get_plugin(plugin_name)
		.ok_or_else(|| anyhow!("Plugin '{}' not found", plugin_name))?;

	log(
		LogLevel::Debug,
		&format!("➜ L7 Executing: {} ({})", plugin_name, flow_path),
	);

	// 2. Execute Middleware
	let output_result: Option<anyhow::Result<MiddlewareOutput>> =
		if let Some(l7_middleware) = plugin.as_l7_middleware() {
			Some(
				l7_middleware
					.execute_l7(container, resolved_inputs.clone())
					.await,
			)
		} else if let Some(middleware) = plugin.as_middleware() {
			Some(middleware.execute(resolved_inputs.clone()).await)
		} else {
			None
		};

	if let Some(result) = output_result {
		let output = result?;
		if let Some(updates) = output.store {
			for (k, v) in updates {
				let scoped_key = plugin_output::format_scoped_key(&flow_path, plugin_name, &k);
				container.kv.insert(scoped_key, v);
			}
		}
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

	// 3. Execute Terminator (L7 Priority > Standard)
	if let Some(l7_terminator) = plugin.as_l7_terminator() {
		return l7_terminator.execute_l7(container, resolved_inputs).await;
	}

	if let Some(terminator) = plugin.as_terminator() {
		let conn_placeholder = ConnectionObject::Virtual("L7_Managed_Context".into());
		return terminator
			.execute(resolved_inputs, &mut container.kv, conn_placeholder)
			.await;
	}

	Err(anyhow!(
		"Plugin '{}' type mismatch: Expected Middleware (L7/Std) or Terminator (L7/Std)",
		plugin_name
	))
}

/// Resolves input templates with "Hijack" logic.
/// Priority:
/// 1. Magic Body Words (req.body) -> Lazy Buffer
/// 2. Magic Header Words (req.header.x) -> Container Lookup (On-Demand)
/// 3. KV Store -> Fallback
async fn resolve_inputs_l7(
	inputs: &HashMap<String, Value>,
	container: &mut Container,
) -> anyhow::Result<HashMap<String, Value>> {
	let mut resolved = inputs.clone();

	for (key, value) in inputs {
		if let Some(s) = value.as_str() {
			if s.starts_with("{{") && s.ends_with("}}") {
				let lookup_key = &s[2..s.len() - 2];

				// PRIORITY 1: Lazy Buffering (Body)
				if matches!(
					lookup_key,
					"req.body" | "req.body_hex" | "res.body" | "res.body_hex"
				) {
					let bytes = if lookup_key.starts_with("req.") {
						container.force_buffer_request().await?
					} else {
						container.force_buffer_response().await?
					};

					let is_hex = lookup_key.ends_with("_hex");
					let val_str = if is_hex {
						hex::encode(bytes)
					} else {
						String::from_utf8_lossy(bytes).to_string()
					};

					resolved.insert(key.clone(), Value::String(val_str));
					continue;
				}

				// PRIORITY 2: On-Demand Header Extraction (Headers)
				// Case A: Single Header Value (e.g. "req.header.user-agent")
				if let Some(header_name) = lookup_key.strip_prefix("req.header.") {
					let val_str = get_header_value(&container.request_headers, header_name);
					resolved.insert(key.clone(), Value::String(val_str));
					continue;
				}
				if let Some(header_name) = lookup_key.strip_prefix("res.header.") {
					let val_str = get_header_value(&container.response_headers, header_name);
					resolved.insert(key.clone(), Value::String(val_str));
					continue;
				}

				// Case B: Full Headers Dump (e.g. "req.headers")
				if lookup_key == "req.headers" {
					let dump = format!("{:?}", container.request_headers);
					resolved.insert(key.clone(), Value::String(dump));
					continue;
				}
				if lookup_key == "res.headers" {
					let dump = format!("{:?}", container.response_headers);
					resolved.insert(key.clone(), Value::String(dump));
					continue;
				}

				// PRIORITY 3: KV Store Fallback
				if let Some(kv_val) = container.kv.get(lookup_key) {
					resolved.insert(key.clone(), Value::String(kv_val.clone()));
				} else {
					log(
						LogLevel::Warn,
						&format!(
							"⚠ Template '{}' not found in Container KV or Headers.",
							lookup_key
						),
					);
				}
			}
		}
	}
	Ok(resolved)
}

/// Helper to safely extract a specific header value from a HeaderMap.
/// Returns the first value as String, or empty string if missing.
/// Handles case-insensitivity via HeaderMap behavior.
fn get_header_value(map: &HeaderMap, key_name: &str) -> String {
	// HeaderMap lookup is case-insensitive for ASCII keys.
	// But we need to handle potential format errors cleanly.
	match map.get(key_name) {
		Some(val) => val.to_str().unwrap_or("").to_string(),
		None => "".to_string(),
	}
}
