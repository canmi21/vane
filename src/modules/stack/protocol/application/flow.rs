/* src/modules/stack/protocol/application/flow.rs */

use super::container::Container;
use super::template;
use crate::modules::{
	kv::plugin_output,
	plugins::{
		model::{ConnectionObject, MiddlewareOutput, ProcessingStep, TerminatorResult},
		registry,
	},
};
use anyhow::{Context, anyhow};
use fancy_log::{LogLevel, log};

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

	// 1. Resolve Inputs (Using new Recursive Template System)
	let resolved_inputs = template::resolve_inputs(&instance.input, container)
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
