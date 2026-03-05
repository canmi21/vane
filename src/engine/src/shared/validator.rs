use crate::engine::interfaces::{Layer, ParamType, ProcessingStep};
use crate::registry;
use serde_json::Value;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use validator::{ValidationError, ValidationErrors};
use vane_primitives::model::{FlowValidationError, Target, validate_target};

/// Recursively validates a flow-based configuration tree.
/// This internal version uses dynamic Strings to avoid memory leaks.
pub fn validate_flow_recursive(
	step: &ProcessingStep,
	layer: Layer,
	protocol: &str,
	path: String,
	ancestors: &mut Vec<String>,
) -> Vec<FlowValidationError> {
	let mut errors = Vec::new();

	if step.len() != 1 {
		errors.push(FlowValidationError {
			path,
			message: "Each processing step must contain exactly one plugin key.".to_owned(),
		});
		return errors;
	}

	let (plugin_name, instance) = step.iter().next().unwrap();
	let current_path =
		if path.is_empty() { plugin_name.clone() } else { format!("{path} -> {plugin_name}") };

	// 1. Cycle Detection (based on instance path, not plugin name)
	if ancestors.contains(&current_path) {
		errors.push(FlowValidationError {
			path: current_path.clone(),
			message: format!(
				"Cycle detected: instance at '{current_path}' calls itself in its output tree."
			),
		});
		return errors;
	}

	let Some(plugin) = registry::get_plugin(plugin_name) else {
		errors.push(FlowValidationError {
			path: current_path.clone(),
			message: format!("Plugin '{plugin_name}' is not registered."),
		});
		return errors;
	};

	// 2. Validate Parameter Types
	validate_plugin_inputs_internal(
		plugin_name,
		&plugin.params(),
		&instance.input,
		&current_path,
		&mut errors,
	);

	// 3. Validate Layer Compatibility (For Terminators)
	if let Some(terminator) = plugin.as_terminator() {
		let supported = terminator.supported_layers();
		if !supported.contains(&layer) {
			errors.push(FlowValidationError {
				path: current_path.clone(),
				message: format!(
					"Plugin '{plugin_name}' is not supported at layer {layer:?}. Supported layers: {supported:?}"
				),
			});
		}
	}

	// 4. Validate Protocol Compatibility (Specific vs Generic)
	let supported_protocols = plugin.supported_protocols();
	let is_generic = plugin.as_generic_middleware().is_some() || plugin.as_middleware().is_some();
	let is_http_specific =
		plugin.as_http_middleware().is_some() || plugin.as_l7_middleware().is_some();

	if !is_generic && is_http_specific {
		let current_proto = protocol.to_lowercase();

		let supports_current = supported_protocols.iter().any(|p| p.to_lowercase() == current_proto);

		if !supports_current {
			errors.push(FlowValidationError {
				path: current_path.clone(),
				message: format!(
					"Plugin '{plugin_name}' is protocol-specific and does not support protocol '{protocol}'. Supported: {supported_protocols:?}"
				),
			});
		}
	}

	// 5. Validate Middleware Outputs and Recursion
	if !instance.output.is_empty() {
		let expected_branches = if let Some(m) = plugin.as_generic_middleware() {
			Some(m.output())
		} else if let Some(m) = plugin.as_http_middleware() {
			Some(m.output())
		} else if let Some(m) = plugin.as_middleware() {
			Some(m.output())
		} else {
			plugin.as_l7_middleware().map(|m| m.output())
		};

		if let Some(branches) = expected_branches {
			validate_middleware_outputs_internal(
				plugin_name,
				&branches,
				&instance.output,
				&current_path,
				&mut errors,
			);
		}

		ancestors.push(current_path.clone());
		for (branch, next_step) in &instance.output {
			let branch_path = format!("{current_path}.{branch}");
			errors.extend(validate_flow_recursive(next_step, layer, protocol, branch_path, ancestors));
		}
		ancestors.pop();
	}

	errors
}

fn validate_plugin_inputs_internal(
	plugin_name: &str,
	param_defs: &[crate::engine::interfaces::ParamDef],
	inputs: &HashMap<String, Value>,
	current_path: &str,
	errors: &mut Vec<FlowValidationError>,
) {
	for input_name in inputs.keys() {
		if !param_defs.iter().any(|p| p.name.as_ref() == input_name.as_str()) {
			errors.push(FlowValidationError {
				path: format!("{current_path}.input.{input_name}"),
				message: format!("Plugin '{plugin_name}' does not accept parameter '{input_name}'."),
			});
		}
	}

	for def in param_defs {
		match inputs.get(def.name.as_ref()) {
			Some(value) => {
				if let Some(s) = value.as_str()
					&& s.starts_with("{{")
					&& s.ends_with("}}")
				{
					continue;
				}

				let is_valid_type = match def.param_type {
					ParamType::Integer => value.is_i64() || value.is_u64(),
					ParamType::Boolean => value.is_boolean(),
					ParamType::String | ParamType::Bytes => value.is_string(),
					ParamType::Map => value.is_object(),
					ParamType::Array => value.is_array(),
					ParamType::Any => true,
				};
				if !is_valid_type {
					errors.push(FlowValidationError {
						path: format!("{}.input.{}", current_path, def.name),
						message: format!("Parameter '{}' must be of type {:?}.", def.name, def.param_type),
					});
				}

				// Deep validation for Target types (IP/Domain/Node)
				if (def.param_type == ParamType::Any || def.param_type == ParamType::Map)
					&& let Ok(target) = serde_json::from_value::<Target>(value.clone())
				{
					errors.extend(validate_target(&target, &format!("{}.input.{}", current_path, def.name)));
				}
			}
			None => {
				if def.required {
					errors.push(FlowValidationError {
						path: format!("{}.input.{}", current_path, def.name),
						message: format!("Required parameter '{}' is missing.", def.name),
					});
				}
			}
		}
	}
}

fn validate_middleware_outputs_internal(
	plugin_name: &str,
	expected_branches: &[Cow<'static, str>],
	outputs: &HashMap<String, ProcessingStep>,
	current_path: &str,
	errors: &mut Vec<FlowValidationError>,
) {
	let expected_set: HashSet<&str> = expected_branches.iter().map(|s| s.as_ref()).collect();
	for branch_name in outputs.keys() {
		if !expected_set.contains(branch_name.as_str()) {
			errors.push(FlowValidationError {
				path: format!("{current_path}.output.{branch_name}"),
				message: format!(
					"Plugin '{plugin_name}' does not have an output branch named '{branch_name}'."
				),
			});
		}
	}
}

/// Compatibility bridge: Main entry point for Flow validation that returns validator::ValidationErrors.
pub fn validate_flow_config(
	step: &ProcessingStep,
	layer: Layer,
	protocol: &str,
) -> Result<(), ValidationErrors> {
	let mut ancestors = Vec::new();
	let errors = validate_flow_recursive(step, layer, protocol, String::new(), &mut ancestors);

	if errors.is_empty() {
		Ok(())
	} else {
		let mut validation_errors = ValidationErrors::new();
		let full_message = errors
			.into_iter()
			.map(|e| format!("[{}] {}", e.path, e.message))
			.collect::<Vec<_>>()
			.join("\n");

		let mut err = ValidationError::new("flow_validation_failed");
		err.message = Some(full_message.into());
		validation_errors.add("flow", err);
		Err(validation_errors)
	}
}

// Legacy validate_tcp_rules / validate_udp_rules live in crate::config::types
