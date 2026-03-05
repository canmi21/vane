/* src/engine/src/engine/executor.rs */

use anyhow::{Context, Result, anyhow};
use fancy_log::{LogLevel, log};

use crate::{
	engine::interfaces::{ConnectionObject, MiddlewareOutput, ProcessingStep, TerminatorResult},
	registry,
};

use crate::engine::{context::ExecutionContext, key_scoping};

/// Execute a flow starting from the given step.
///
/// This is the unified entry point for all layers (L4, L4+, L7).
///
/// # Parameters
/// - `step`: The ProcessingStep to execute
/// - `context`: Layer-specific execution context
/// - `conn`: Connection object (real for L4/L4+, virtual for L7)
/// - `flow_path`: Current flow path for KV scoping
///
/// # Returns
/// TerminatorResult when flow completes
pub async fn execute<C: ExecutionContext>(
	step: &ProcessingStep,
	context: &mut C,
	conn: ConnectionObject,
	flow_path: String,
) -> Result<TerminatorResult> {
	let timeout_secs = envflag::get::<u64>("FLOW_EXECUTION_TIMEOUT_SECS", 10);

	if let Ok(result) = tokio::time::timeout(
		std::time::Duration::from_secs(timeout_secs),
		execute_recursive(step, context, conn, flow_path),
	)
	.await
	{
		result
	} else {
		log(LogLevel::Error, &format!("✗ Flow execution timed out after {timeout_secs}s"));
		Err(anyhow!("Flow execution timeout"))
	}
}

/// Recursive flow execution (internal)
async fn execute_recursive<C: ExecutionContext>(
	step: &ProcessingStep,
	context: &mut C,
	conn: ConnectionObject,
	flow_path: String,
) -> Result<TerminatorResult> {
	// 1. Parse step (exactly one plugin per step)
	if step.len() != 1 {
		return Err(anyhow!("Invalid step: expected exactly 1 plugin, found {}", step.len()));
	}

	let (plugin_name, instance) =
		step.iter().next().ok_or_else(|| anyhow!("Empty processing step"))?;

	// 2. Resolve template inputs (delegated to context)
	let resolved_inputs = context.resolve_inputs(&instance.input).await;

	// 3. Get plugin from registry
	let plugin = registry::get_plugin(plugin_name)
		.ok_or_else(|| anyhow!("Plugin '{plugin_name}' not found in registry"))?;

	log(LogLevel::Debug, &format!("➜ Executing plugin: {plugin_name} (Path: '{flow_path}')"));

	// --- Passive Circuit Breaker (for External Plugins) ---
	let is_external = registry::get_external_plugin(plugin_name).is_some();
	if is_external && let Some(last_failure) = registry::EXTERNAL_PLUGIN_FAILURES.get(plugin_name) {
		let quiet_period_secs = envflag::get::<u64>("EXTERNAL_PLUGIN_QUIET_PERIOD_SECS", 3);

		if last_failure.elapsed().as_secs() < quiet_period_secs {
			log(
				LogLevel::Warn,
				&format!(
					"➜ Circuit Breaker: Plugin '{plugin_name}' is in quiet period (last failure < {quiet_period_secs}s ago). Skipping IO and returning failure branch."
				),
			);
			// Fast-fail: return failure branch with metadata
			let output = MiddlewareOutput {
				branch: "failure".into(),
				store: Some(std::collections::HashMap::from([(
					"error".to_owned(),
					"circuit_breaker_active".to_owned(),
				)])),
			};
			// We proceed to handle this as a standard middleware output
			return handle_middleware_output(output, plugin_name, &flow_path, instance, context, conn)
				.await;
		}
	}

	// 4. Try dispatch (Priority: Middleware > Terminator)
	let output_res = if let Some(http_middleware) = plugin.as_http_middleware() {
		// 4.1 Protocol-Specific HTTP (Internal only)
		http_middleware
			.execute(context.as_any_mut(), resolved_inputs)
			.await
			.with_context(|| format!("Error executing HTTP middleware '{plugin_name}'"))
	} else if let Some(generic_middleware) = plugin.as_generic_middleware() {
		// 4.2 Generic Middleware (Internal or External)
		generic_middleware
			.execute(resolved_inputs)
			.await
			.with_context(|| format!("Error executing generic middleware '{plugin_name}'"))
	} else if let Some(l7_middleware) = plugin.as_l7_middleware() {
		// 4.3 Legacy L7 Fallback
		l7_middleware
			.execute_l7(context.as_any_mut(), resolved_inputs)
			.await
			.with_context(|| format!("Error executing L7 middleware '{plugin_name}'"))
	} else if let Some(middleware) = plugin.as_middleware() {
		// 4.4 Legacy generic Fallback
		middleware
			.execute(resolved_inputs)
			.await
			.with_context(|| format!("Error executing middleware '{plugin_name}'"))
	} else {
		// 5. Try terminator dispatch (L7 Priority > Standard)
		let terminator_result = if let Some(l7_terminator) = plugin.as_l7_terminator() {
			l7_terminator
				.execute_l7(context.as_any_mut(), resolved_inputs)
				.await
				.with_context(|| format!("Error executing L7 terminator '{plugin_name}'"))?
		} else if let Some(terminator) = plugin.as_terminator() {
			terminator
				.execute(resolved_inputs, context.kv_mut(), conn)
				.await
				.with_context(|| format!("Error executing terminator '{plugin_name}'"))?
		} else {
			return Err(anyhow!("Plugin '{plugin_name}' is neither Middleware nor Terminator"));
		};

		match &terminator_result {
			TerminatorResult::Finished => {
				log(LogLevel::Debug, &format!("✓ Flow terminated successfully by '{plugin_name}'"));
			}
			TerminatorResult::Upgrade { protocol, .. } => {
				log(
					LogLevel::Info,
					&format!("➜ Flow upgrade requested by '{plugin_name}' -> Protocol: {protocol}"),
				);
			}
		}

		return Ok(terminator_result);
	};

	// 6. Check for runtime errors and update circuit breaker
	let output = match output_res {
		Ok(out) => {
			if is_external && out.branch == "failure" {
				log(
					LogLevel::Warn,
					&format!(
						"⚠ External plugin '{plugin_name}' returned 'failure' branch. Marking as failed in Circuit Breaker."
					),
				);
				registry::EXTERNAL_PLUGIN_FAILURES.insert(plugin_name.clone(), std::time::Instant::now());
			}
			out
		}
		Err(e) => {
			if is_external {
				log(
					LogLevel::Error,
					&format!(
						"✗ Runtime error in external plugin '{plugin_name}': {e}. Activating quiet period."
					),
				);
				registry::EXTERNAL_PLUGIN_FAILURES.insert(plugin_name.clone(), std::time::Instant::now());
			}
			return Err(e);
		}
	};

	handle_middleware_output(output, plugin_name, &flow_path, instance, context, conn).await
}

/// Extracted helper to handle middleware success/failure branches
async fn handle_middleware_output<C: ExecutionContext>(
	output: MiddlewareOutput,
	plugin_name: &str,
	flow_path: &str,
	instance: &crate::engine::interfaces::PluginInstance,
	context: &mut C,
	conn: ConnectionObject,
) -> Result<TerminatorResult> {
	log(
		LogLevel::Debug,
		&format!("✓ Middleware '{}' returned branch: '{}'", plugin_name, output.branch),
	);

	// Store KV updates with scoped keys
	if let Some(updates) = output.store {
		let kv = context.kv_mut();
		for (raw_key, value) in updates.into_iter() {
			// Security: Validate key name to prevent template injection risks
			if raw_key.contains('{') || raw_key.contains('}') {
				log(
					LogLevel::Error,
					&format!(
						"✗ Security: Plugin '{plugin_name}' attempted to store an invalid key name containing '{{' or '}}'. Ignoring: '{raw_key}'"
					),
				);
				continue;
			}

			let scoped_key = key_scoping::format_scoped_key(flow_path, plugin_name, &raw_key);
			log(LogLevel::Debug, &format!("⚙ KV Update: {scoped_key} = {value}"));
			kv.insert(scoped_key, value);
		}
	}

	// Branch to next step based on output
	if let Some(next_step) = instance.output.get(output.branch.as_ref()) {
		let next_path = key_scoping::next_path(flow_path, plugin_name, output.branch.as_ref());
		Box::pin(execute_recursive(next_step, context, conn, next_path)).await
	} else {
		Err(anyhow!(
			"Flow stalled at '{}': branch '{}' not configured in output",
			plugin_name,
			output.branch
		))
	}
}
