/* src/modules/flow/engine.rs */

use anyhow::{Context, Result, anyhow};
use fancy_log::{LogLevel, log};

use crate::modules::{
	plugins::model::{ConnectionObject, MiddlewareOutput, ProcessingStep, TerminatorResult},
	plugins::registry,
	stack::protocol::application::container::Container,
};

use super::{context::ApplicationContext, context::ExecutionContext, key_scoping};

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
	execute_recursive(step, context, conn, flow_path).await
}

/// Convenience wrapper for L7 flow execution.
pub async fn execute_l7(
	step: &ProcessingStep,
	container: &mut Container,
	flow_path: String,
) -> Result<TerminatorResult> {
	let mut context = ApplicationContext { container };
	let conn = ConnectionObject::Virtual("L7_Managed_Context".into());
	execute(step, &mut context, conn, flow_path).await
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
		return Err(anyhow!(
			"Invalid step: expected exactly 1 plugin, found {}",
			step.len()
		));
	}

	let (plugin_name, instance) = step
		.iter()
		.next()
		.ok_or_else(|| anyhow!("Empty processing step"))?;

	// 2. Resolve template inputs (delegated to context)
	let resolved_inputs = context.resolve_inputs(&instance.input).await;

	// 3. Get plugin from registry
	let plugin = registry::get_plugin(plugin_name)
		.ok_or_else(|| anyhow!("Plugin '{}' not found in registry", plugin_name))?;

	log(
		LogLevel::Debug,
		&format!(
			"➜ Executing plugin: {} (Path: '{}')",
			plugin_name, flow_path
		),
	);

	// 4. Try middleware dispatch (Priority: Http > Generic > Legacy)
	let output_result: Option<Result<MiddlewareOutput>> =
		if let Some(http_middleware) = plugin.as_http_middleware() {
			// 4.1 Protocol-Specific HTTP (Internal only)
			Some(
				http_middleware
					.execute(context.as_any_mut(), resolved_inputs.clone())
					.await
					.with_context(|| format!("Error executing HTTP middleware '{}'", plugin_name)),
			)
		} else if let Some(generic_middleware) = plugin.as_generic_middleware() {
			// 4.2 Generic Middleware (Internal or External)
			Some(
				generic_middleware
					.execute(resolved_inputs.clone())
					.await
					.with_context(|| format!("Error executing generic middleware '{}'", plugin_name)),
			)
		} else if let Some(l7_middleware) = plugin.as_l7_middleware() {
			// 4.3 Legacy L7 Fallback
			Some(
				l7_middleware
					.execute_l7(context.as_any_mut(), resolved_inputs.clone())
					.await
					.with_context(|| format!("Error executing L7 middleware '{}'", plugin_name)),
			)
		} else if let Some(middleware) = plugin.as_middleware() {
			// 4.4 Legacy generic Fallback
			Some(
				middleware
					.execute(resolved_inputs.clone())
					.await
					.with_context(|| format!("Error executing middleware '{}'", plugin_name)),
			)
		} else {
			None
		};

	if let Some(result) = output_result {
		let output = result?;

		log(
			LogLevel::Debug,
			&format!(
				"✓ Middleware '{}' returned branch: '{}'",
				plugin_name, output.branch
			),
		);

		// Store KV updates with scoped keys
		if let Some(updates) = output.store {
			let kv = context.kv_mut();
			for (raw_key, value) in updates {
				let scoped_key = key_scoping::format_scoped_key(&flow_path, plugin_name, &raw_key);
				log(
					LogLevel::Debug,
					&format!("⚙ KV Update: {} = {}", scoped_key, value),
				);
				kv.insert(scoped_key, value);
			}
		}

		// Branch to next step based on output
		if let Some(next_step) = instance.output.get(output.branch.as_ref()) {
			let next_path = key_scoping::next_path(&flow_path, plugin_name, output.branch.as_ref());
			return Box::pin(execute_recursive(next_step, context, conn, next_path)).await;
		} else {
			return Err(anyhow!(
				"Flow stalled at '{}': branch '{}' not configured in output",
				plugin_name,
				output.branch
			));
		}
	}

	// 5. Try terminator dispatch (L7 Priority > Standard)
	let terminator_result = if let Some(l7_terminator) = plugin.as_l7_terminator() {
		l7_terminator
			.execute_l7(context.as_any_mut(), resolved_inputs)
			.await
			.with_context(|| format!("Error executing L7 terminator '{}'", plugin_name))?
	} else if let Some(terminator) = plugin.as_terminator() {
		terminator
			.execute(resolved_inputs, context.kv_mut(), conn)
			.await
			.with_context(|| format!("Error executing terminator '{}'", plugin_name))?
	} else {
		return Err(anyhow!(
			"Plugin '{}' is neither Middleware nor Terminator",
			plugin_name
		));
	};

	match &terminator_result {
		TerminatorResult::Finished => {
			log(
				LogLevel::Debug,
				&format!("✓ Flow terminated successfully by '{}'", plugin_name),
			);
		}
		TerminatorResult::Upgrade { protocol, .. } => {
			log(
				LogLevel::Info,
				&format!(
					"➜ Flow upgrade requested by '{}' -> Protocol: {}",
					plugin_name, protocol
				),
			);
		}
	}

	Ok(terminator_result)
}
