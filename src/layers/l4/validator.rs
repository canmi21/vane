/* src/layers/l4/validator.rs */

use crate::engine::interfaces::{Layer, ParamType, ProcessingStep};
use crate::plugins::core::registry;
use serde_json::Value;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use validator::{ValidationError, ValidationErrors};

use super::model::Target;
use crate::layers::l4::legacy::{tcp::TcpProtocolRule, udp::UdpProtocolRule};

/// Internal error type for flow validation that doesn't require &'static str.
#[derive(Debug)]
pub struct FlowValidationError {
	pub path: String,
	pub message: String,
}

pub fn validate_target(target: &Target, path: &str) -> Vec<FlowValidationError> {
	let mut errors = Vec::new();
	match target {
		Target::Domain { domain, .. } => {
			if !cfg!(feature = "domain-target") {
				errors.push(FlowValidationError {
					path: path.to_string(),
					message: format!(
						"Domain target '{}' is disabled in this build. Please recompile with 'domain-target' feature enabled.",
						domain
					),
				});
			}
		}
		_ => {}
	}
	errors
}

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
			path: path.clone(),
			message: "Each processing step must contain exactly one plugin key.".to_string(),
		});
		return errors;
	}

	let (plugin_name, instance) = step.iter().next().unwrap();
	let current_path = if path.is_empty() {
		plugin_name.clone()
	} else {
		format!("{} -> {}", path, plugin_name)
	};

	// 0. Check Feature Constraints for Built-in Plugins
	if plugin_name.starts_with("internal.") {
		let is_disabled = match plugin_name.as_str() {
			"internal.driver.cgi" => !cfg!(feature = "cgi"),
			"internal.driver.static" => !cfg!(feature = "static"),
			"internal.common.ratelimit.sec" | "internal.common.ratelimit.min" => {
				!cfg!(feature = "ratelimit")
			}
			"internal.driver.upstream" => !cfg!(any(feature = "h2upstream", feature = "h3upstream")),
			_ => false,
		};

		if is_disabled {
			errors.push(FlowValidationError {
				path: current_path.clone(),
				message: format!(
					"Plugin '{}' is disabled in this build. Please recompile Vane with the corresponding feature enabled.",
					plugin_name
				),
			});
			return errors;
		}
	}

	// 1. Cycle Detection
	if ancestors.contains(plugin_name) {
		errors.push(FlowValidationError {
			path: current_path.clone(),
			message: format!(
				"Cycle detected: plugin '{}' calls itself in its output tree.",
				plugin_name
			),
		});
		return errors;
	}

	let plugin = match registry::get_plugin(plugin_name) {
		Some(p) => p,
		None => {
			errors.push(FlowValidationError {
				path: current_path.clone(),
				message: format!("Plugin '{}' is not registered.", plugin_name),
			});
			return errors;
		}
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
					"Plugin '{}' is not supported at layer {:?}. Supported layers: {:?}",
					plugin_name, layer, supported
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

		// Check if the protocol itself is disabled via features
		let proto_disabled = match current_proto.as_str() {
			"tls" => !cfg!(feature = "tls"),
			"quic" => !cfg!(feature = "quic"),
			"httpx" => !cfg!(feature = "httpx"),
			_ => false,
		};

		if proto_disabled {
			errors.push(FlowValidationError {
				path: current_path.clone(),
				message: format!(
					"Protocol '{}' is disabled in this build. Please recompile Vane with the corresponding feature enabled.",
					current_proto
				),
			});
			return errors;
		}

		let supports_current = supported_protocols
			.iter()
			.any(|p| p.to_lowercase() == current_proto);

		if !supports_current {
			errors.push(FlowValidationError {
				path: current_path.clone(),
				message: format!(
					"Plugin '{}' is protocol-specific and does not support protocol '{}'. Supported: {:?}",
					plugin_name, protocol, supported_protocols
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
		} else if let Some(m) = plugin.as_l7_middleware() {
			Some(m.output())
		} else {
			None
		};

		if let Some(branches) = expected_branches {
			validate_middleware_outputs_internal(
				plugin_name,
				branches,
				&instance.output,
				&current_path,
				&mut errors,
			);
		}

		ancestors.push(plugin_name.clone());
		for (branch, next_step) in &instance.output {
			let branch_path = format!("{}.{}", current_path, branch);
			errors.extend(validate_flow_recursive(
				next_step,
				layer,
				protocol,
				branch_path,
				ancestors,
			));
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
		if !param_defs
			.iter()
			.any(|p| p.name.as_ref() == input_name.as_str())
		{
			errors.push(FlowValidationError {
				path: format!("{}.input.{}", current_path, input_name),
				message: format!(
					"Plugin '{}' does not accept parameter '{}'.",
					plugin_name, input_name
				),
			});
		}
	}

	for def in param_defs {
		match inputs.get(def.name.as_ref()) {
			Some(value) => {
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
					ParamType::Any => true,
				};
				if !is_valid_type {
					errors.push(FlowValidationError {
						path: format!("{}.input.{}", current_path, def.name),
						message: format!(
							"Parameter '{}' must be of type {:?}.",
							def.name, def.param_type
						),
					});
				}

				// Deep validation for Target types (IP/Domain/Node)
				if def.param_type == ParamType::Any || def.param_type == ParamType::Map {
					if let Ok(target) = serde_json::from_value::<Target>(value.clone()) {
						errors.extend(validate_target(
							&target,
							&format!("{}.input.{}", current_path, def.name),
						));
					}
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
	expected_branches: Vec<Cow<'static, str>>,
	outputs: &HashMap<String, ProcessingStep>,
	current_path: &str,
	errors: &mut Vec<FlowValidationError>,
) {
	let expected_set: HashSet<&str> = expected_branches.iter().map(|s| s.as_ref()).collect();

	for branch_name in outputs.keys() {
		if !expected_set.contains(branch_name.as_str()) {
			errors.push(FlowValidationError {
				path: format!("{}.output.{}", current_path, branch_name),
				message: format!(
					"Plugin '{}' does not have an output branch named '{}'.",
					plugin_name, branch_name
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
		// We flatten all errors into a single multiline message under a static key
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
