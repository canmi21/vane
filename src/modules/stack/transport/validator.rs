/* src/modules/stack/transport/validator.rs */

use crate::modules::plugins::{
	model::{Layer, ParamType, ProcessingStep},
	registry,
};
use serde_json::Value;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use validator::{ValidationError, ValidationErrors};

use super::tcp::TcpProtocolRule;
use super::udp::UdpProtocolRule;

/// Recursively validates a flow-based configuration tree.
pub fn validate_flow_config(step: &ProcessingStep, layer: Layer) -> Result<(), ValidationErrors> {
	if step.len() != 1 {
		let mut err = ValidationError::new("processing_step_size");
		err.message = Some("Each processing step must contain exactly one plugin key.".into());
		let mut errors = ValidationErrors::new();
		errors.add("step", err);
		return Err(errors);
	}

	let (plugin_name, instance) = step.iter().next().unwrap();
	let mut errors = ValidationErrors::new();

	let plugin = match registry::get_plugin(plugin_name) {
		Some(p) => p,
		None => {
			let mut err = ValidationError::new("unknown_plugin");
			err.message = Some(format!("Plugin '{}' is not registered.", plugin_name).into());
			errors.add(Box::leak(plugin_name.clone().into_boxed_str()), err);
			return Err(errors);
		}
	};

	// 1. Validate Parameter Types
	if let Err(e) = validate_plugin_inputs(plugin_name, &plugin.params(), &instance.input) {
		errors.merge_self("input", Err(e));
	}

	// 2. Validate Layer Compatibility (For Terminators)
	if let Some(terminator) = plugin.as_terminator() {
		let supported = terminator.supported_layers();
		if !supported.contains(&layer) {
			let mut err = ValidationError::new("invalid_layer");
			err.message = Some(
				format!(
					"Plugin '{}' is not supported at layer {:?}. Supported layers: {:?}",
					plugin_name, layer, supported
				)
				.into(),
			);
			errors.add(Box::leak(plugin_name.clone().into_boxed_str()), err);
		}
	}

	// 3. Validate Middleware Outputs and Recursion
	if !instance.output.is_empty() {
		if let Some(middleware) = plugin.as_middleware() {
			if let Err(e) =
				validate_middleware_outputs(plugin_name, middleware.output(), &instance.output)
			{
				errors.merge_self("output", Err(e));
			}
		}

		for (_branch, next_step) in &instance.output {
			if let Err(e) = validate_flow_config(next_step, layer) {
				errors.merge_self("output", Err(e));
			}
		}
	}

	if errors.is_empty() {
		Ok(())
	} else {
		Err(errors)
	}
}

fn validate_plugin_inputs(
	plugin_name: &str,
	param_defs: &[super::super::super::plugins::model::ParamDef],
	inputs: &HashMap<String, Value>,
) -> Result<(), ValidationErrors> {
	let mut errors = ValidationErrors::new();

	for input_name in inputs.keys() {
		if !param_defs
			.iter()
			.any(|p| p.name.as_ref() == input_name.as_str())
		{
			let mut err = ValidationError::new("unknown_parameter");
			err.message = Some(
				format!(
					"Plugin '{}' does not accept parameter '{}'.",
					plugin_name, input_name
				)
				.into(),
			);
			errors.add(Box::leak(input_name.clone().into_boxed_str()), err);
		}
	}

	for def in param_defs {
		match inputs.get(def.name.as_ref()) {
			Some(value) => {
				// Skip type validation for template strings {{...}}
				// Note: For Map/Array/Any, templates might be nested inside.
				// A deep validation of templates is complex, so we skip basic string templates at top level.
				if let Some(s) = value.as_str() {
					if s.starts_with("{{") && s.ends_with("}}") {
						continue;
					}
				}

				let is_valid_type = match def.param_type {
					ParamType::String => value.is_string(),
					ParamType::Integer => value.is_i64() || value.is_u64(),
					ParamType::Boolean => value.is_boolean(),
					ParamType::Bytes => value.is_string(),
					ParamType::Map => value.is_object(),
					ParamType::Array => value.is_array(),
					ParamType::Any => true, // Accepts anything
				};
				if !is_valid_type {
					let mut err = ValidationError::new("invalid_parameter_type");
					err.message = Some(
						format!(
							"Parameter '{}' must be of type {:?}.",
							def.name, def.param_type
						)
						.into(),
					);
					errors.add(Box::leak(def.name.to_string().into_boxed_str()), err);
				}
			}
			None => {
				if def.required {
					let mut err = ValidationError::new("required_parameter_missing");
					err.message = Some(format!("Required parameter '{}' is missing.", def.name).into());
					errors.add(Box::leak(def.name.to_string().into_boxed_str()), err);
				}
			}
		}
	}

	if errors.is_empty() {
		Ok(())
	} else {
		Err(errors)
	}
}

fn validate_middleware_outputs(
	plugin_name: &str,
	expected_branches: Vec<Cow<'static, str>>,
	outputs: &HashMap<String, ProcessingStep>,
) -> Result<(), ValidationErrors> {
	let mut errors = ValidationErrors::new();
	let expected_set: HashSet<&str> = expected_branches.iter().map(|s| s.as_ref()).collect();

	for branch_name in outputs.keys() {
		if !expected_set.contains(branch_name.as_str()) {
			let mut err = ValidationError::new("unknown_output_branch");
			err.message = Some(
				format!(
					"Plugin '{}' does not have an output branch named '{}'.",
					plugin_name, branch_name
				)
				.into(),
			);
			errors.add("output", err);
		}
	}

	if errors.is_empty() {
		Ok(())
	} else {
		Err(errors)
	}
}

// --- Existing Legacy Validators ---

pub fn validate_tcp_rules(rules: &[TcpProtocolRule]) -> Result<(), ValidationError> {
	let mut priorities = HashSet::new();
	for rule in rules {
		if !priorities.insert(rule.priority) {
			let mut err = ValidationError::new("unique_priorities");
			err.message = Some("Priorities must be unique within a listener config.".into());
			return Err(err);
		}
	}
	Ok(())
}

pub fn validate_udp_rules(rules: &[UdpProtocolRule]) -> Result<(), ValidationError> {
	let mut priorities = HashSet::new();
	for rule in rules {
		if !priorities.insert(rule.priority) {
			let mut err = ValidationError::new("unique_priorities");
			err.message = Some("Priorities must be unique within a listener config.".into());
			return Err(err);
		}
	}
	Ok(())
}
