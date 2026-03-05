/* src/layers/l4/validator.rs */

use crate::engine::interfaces::{Layer, ParamType, ProcessingStep};
use crate::plugins::core::registry;
use serde_json::Value;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use validator::{ValidationError, ValidationErrors};

use super::model::{FlowValidationError, Target, validate_target};
use crate::layers::l4::legacy::{tcp::TcpProtocolRule, udp::UdpProtocolRule};

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
	let current_path = if path.is_empty() {
		plugin_name.clone()
	} else {
		format!("{path} -> {plugin_name}")
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
					"Plugin '{plugin_name}' is disabled in this build. Please recompile Vane with the corresponding feature enabled."
				),
			});
			return errors;
		}
	}

	// 1. Cycle Detection (based on instance path, not plugin name)
	// A cycle occurs when an instance's output tree eventually leads back to that same instance,
	// not when the same plugin type appears multiple times in different positions.
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
					"Protocol '{current_proto}' is disabled in this build. Please recompile Vane with the corresponding feature enabled."
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
						message: format!(
							"Parameter '{}' must be of type {:?}.",
							def.name, def.param_type
						),
					});
				}

				// Deep validation for Target types (IP/Domain/Node)
				if (def.param_type == ParamType::Any || def.param_type == ParamType::Map)
					&& let Ok(target) = serde_json::from_value::<Target>(value.clone())
				{
					errors.extend(validate_target(
						&target,
						&format!("{}.input.{}", current_path, def.name),
					));
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

#[cfg(test)]
mod tests {
	use crate::layers::l4::legacy::tcp::{TcpDestination, TcpProtocolRule, TcpSession};
	use crate::layers::l4::model::{Detect, DetectMethod, Forward, Strategy, Target};
	use validator::Validate;

	#[test]
	fn test_validate_target_port_range() {
		// 1. Valid Port
		let valid = Target::Ip {
			ip: "127.0.0.1".to_string(),
			port: 8080,
		};
		assert!(valid.validate().is_ok());

		// 2. Invalid Port 0
		let invalid = Target::Ip {
			ip: "127.0.0.1".to_string(),
			port: 0,
		};
		let res = invalid.validate();
		assert!(res.is_err());
		let errs = res.unwrap_err();
		assert!(errs.field_errors().contains_key("port"));
	}

	#[test]
	fn test_validate_timeout_value() {
		// 1. Valid Timeout
		let session_valid = TcpSession {
			keepalive: true,
			timeout: 30,
		};
		assert!(session_valid.validate().is_ok());

		// 2. Invalid Timeout 0
		let session_invalid = TcpSession {
			keepalive: true,
			timeout: 0,
		};
		let res = session_invalid.validate();
		assert!(res.is_err());
		assert!(res.unwrap_err().field_errors().contains_key("timeout"));
	}

	#[test]
	fn test_validate_tcp_rule_nested() {
		let rule = TcpProtocolRule {
			name: "test_rule".to_string(),
			priority: 1,
			detect: Detect {
				method: DetectMethod::Fallback,
				pattern: "any".to_string(),
			},
			session: Some(TcpSession {
				keepalive: true,
				timeout: 0, // Invalid!
			}),
			destination: TcpDestination::Forward {
				forward: Forward {
					strategy: Strategy::Random,
					targets: vec![Target::Ip {
						ip: "1.1.1.1".into(),
						port: 80,
					}],
					fallbacks: vec![],
				},
			},
		};

		let res = rule.validate();
		assert!(res.is_err());
		// Error path should be session.timeout ideally, or session
		// Validator nested errors structure is complex, just checking it fails is enough for unit test.
	}
}
