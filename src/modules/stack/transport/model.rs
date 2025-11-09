/* src/modules/stack/transport/model.rs */

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::net::IpAddr;
use validator::{Validate, ValidationError, ValidationErrors};

/// The final, resolved representation of a target: a concrete IP and port.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedTarget {
	pub ip: String,
	pub port: u16,
}

/// Represents a target in the configuration file, which can be an IP, domain, or node.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
#[serde(untagged)]
pub enum Target {
	Ip { ip: String, port: u16 },
	Domain { domain: String, port: u16 },
	Node { node: String, port: u16 },
}

impl Validate for Target {
	fn validate(&self) -> Result<(), ValidationErrors> {
		let mut errors = ValidationErrors::new();
		match self {
			Target::Ip { ip, .. } => {
				if ip.parse::<IpAddr>().is_err() {
					errors.add("ip", ValidationError::new("ip"));
				}
			}
			Target::Domain { domain, .. } => {
				if domain.is_empty() || domain.len() > 253 {
					errors.add("domain", ValidationError::new("hostname"));
				}
			}
			Target::Node { .. } => { /* Node name validity is checked implicitly */ }
		}
		if errors.is_empty() {
			Ok(())
		} else {
			Err(errors)
		}
	}
}

/// The method used for L4 protocol detection.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DetectMethod {
	Magic,
	Prefix,
	Regex,
	Fallback,
}

#[derive(Serialize, Deserialize, Debug, Clone, Validate, PartialEq, Eq)]
pub struct Detect {
	pub method: DetectMethod,
	#[validate(length(min = 1))]
	pub pattern: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Strategy {
	Random,
	Serial,
	Fastest,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Forward {
	pub strategy: Strategy,
	#[serde(default)]
	pub targets: Vec<Target>,
	#[serde(default)]
	pub fallbacks: Vec<Target>,
}

impl Validate for Forward {
	fn validate(&self) -> Result<(), ValidationErrors> {
		let mut errors = ValidationErrors::new();

		if self.targets.is_empty() {
			let mut err = ValidationError::new("length");
			err.message = Some("must have at least one target".into());
			errors.add("targets", err);
		}

		for (i, target) in self.targets.iter().enumerate() {
			let field_name = Box::leak(format!("targets[{}]", i).into_boxed_str());
			errors.merge_self(field_name, target.validate());
		}

		for (i, target) in self.fallbacks.iter().enumerate() {
			let field_name = Box::leak(format!("fallbacks[{}]", i).into_boxed_str());
			errors.merge_self(field_name, target.validate());
		}

		if errors.is_empty() {
			Ok(())
		} else {
			Err(errors)
		}
	}
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct TcpSession {
	pub keepalive: bool,
	pub timeout: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TcpDestination {
	Resolver { resolver: String },
	Forward { forward: Forward },
}

impl Validate for TcpDestination {
	fn validate(&self) -> Result<(), ValidationErrors> {
		match self {
			TcpDestination::Resolver { .. } => Ok(()),
			TcpDestination::Forward { forward } => forward.validate(),
		}
	}
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UdpDestination {
	Resolver { resolver: String },
	Forward { forward: Forward },
}

impl Validate for UdpDestination {
	fn validate(&self) -> Result<(), ValidationErrors> {
		match self {
			UdpDestination::Resolver { .. } => Ok(()),
			UdpDestination::Forward { forward } => forward.validate(),
		}
	}
}

lazy_static::lazy_static! {
	static ref NAME_REGEX: regex::Regex = regex::Regex::new(r"^[a-z0-9]+$").unwrap();
}

#[derive(Serialize, Deserialize, Debug, Clone, Validate, PartialEq, Eq)]
pub struct TcpProtocolRule {
	#[validate(regex(path = *NAME_REGEX, message = "can only contain lowercase letters and numbers"))]
	pub name: String,
	#[validate(range(min = 1))]
	pub priority: u32,
	#[validate(nested)]
	pub detect: Detect,
	#[serde(default)]
	pub session: Option<TcpSession>,
	#[validate(nested)]
	pub destination: TcpDestination,
}

#[derive(Serialize, Deserialize, Debug, Clone, Validate, PartialEq, Eq)]
pub struct UdpProtocolRule {
	#[validate(regex(path = *NAME_REGEX, message = "can only contain lowercase letters and numbers"))]
	pub name: String,
	#[validate(range(min = 1))]
	pub priority: u32,
	#[validate(nested)]
	pub detect: Detect,
	#[validate(nested)]
	pub destination: UdpDestination,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct TcpConfig {
	#[serde(rename = "protocols")]
	pub rules: Vec<TcpProtocolRule>,
}

impl Validate for TcpConfig {
	fn validate(&self) -> Result<(), ValidationErrors> {
		let mut result: Result<(), ValidationErrors> = Ok(());
		for (i, rule) in self.rules.iter().enumerate() {
			let field_name = Box::leak(format!("rules[{}]", i).into_boxed_str());
			result = ValidationErrors::merge(result, field_name, rule.validate());
		}
		if let Err(e) = validate_tcp_rules(&self.rules) {
			if let Err(ref mut errors) = result {
				errors.add("rules", e);
			} else {
				let mut errors = ValidationErrors::new();
				errors.add("rules", e);
				result = Err(errors);
			}
		}
		result
	}
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct UdpConfig {
	#[serde(rename = "protocols")]
	pub rules: Vec<UdpProtocolRule>,
}

impl Validate for UdpConfig {
	fn validate(&self) -> Result<(), ValidationErrors> {
		let mut result: Result<(), ValidationErrors> = Ok(());
		for (i, rule) in self.rules.iter().enumerate() {
			let field_name = Box::leak(format!("rules[{}]", i).into_boxed_str());
			result = ValidationErrors::merge(result, field_name, rule.validate());
		}
		if let Err(e) = validate_udp_rules(&self.rules) {
			if let Err(ref mut errors) = result {
				errors.add("rules", e);
			} else {
				let mut errors = ValidationErrors::new();
				errors.add("rules", e);
				result = Err(errors);
			}
		}
		result
	}
}

pub fn validate_tcp_rules(rules: &[TcpProtocolRule]) -> Result<(), ValidationError> {
	let mut priorities = HashSet::new();
	for rule in rules {
		if !priorities.insert(rule.priority) {
			let mut err = ValidationError::new("unique_priorities");
			err.message = Some("Priorities must be unique within a listener config.".into());
			return Err(err);
		}
		if rule.session.is_some() {
			if let TcpDestination::Forward { .. } = &rule.destination {
				let mut err = ValidationError::new("session_with_forward");
				err.message =
					Some("The 'session' block is only allowed for 'resolver' type destinations.".into());
				return Err(err);
			}
		}
		match rule.detect.method {
			DetectMethod::Regex => {
				if fancy_regex::Regex::new(&rule.detect.pattern).is_err() {
					let mut err = ValidationError::new("invalid_regex");
					err.message =
						Some(format!("Pattern '{}' is not a valid regex.", rule.detect.pattern).into());
					return Err(err);
				}
			}
			DetectMethod::Fallback => {
				if rule.detect.pattern != "any" {
					let mut err = ValidationError::new("invalid_fallback_pattern");
					err.message = Some("Pattern for 'fallback' method must be 'any'.".into());
					return Err(err);
				}
			}
			_ => {}
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
		match rule.detect.method {
			DetectMethod::Regex => {
				if fancy_regex::Regex::new(&rule.detect.pattern).is_err() {
					let mut err = ValidationError::new("invalid_regex");
					err.message =
						Some(format!("Pattern '{}' is not a valid regex.", rule.detect.pattern).into());
					return Err(err);
				}
			}
			DetectMethod::Fallback => {
				if rule.detect.pattern != "any" {
					let mut err = ValidationError::new("invalid_fallback_pattern");
					err.message = Some("Pattern for 'fallback' method must be 'any'.".into());
					return Err(err);
				}
			}
			_ => {}
		}
	}
	Ok(())
}
