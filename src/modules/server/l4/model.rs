/* src/modules/server/l4/model.rs */

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use validator::{Validate, ValidationError, ValidationErrors};

/// A single forwarding target, containing an IP and port.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Target {
	pub ip: String,
	pub port: u16,
}

/// The method used for L4 protocol detection.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DetectMethod {
	Magic,
	Prefix,
}

/// The configuration for how to detect a protocol.
#[derive(Serialize, Deserialize, Debug, Clone, Validate, PartialEq, Eq)]
pub struct Detect {
	pub method: DetectMethod,
	#[validate(length(min = 1))]
	pub pattern: String,
}

/// The load balancing strategy for forwarders.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Strategy {
	Random,
	Serial,
	Fastest,
}

/// Configuration for L4 forwarding (shared by TCP and UDP).
#[derive(Serialize, Deserialize, Debug, Clone, Validate, PartialEq, Eq)]
pub struct Forward {
	pub strategy: Strategy,
	#[validate(length(min = 1, message = "must have at least one target"))]
	pub targets: Vec<Target>,
	#[serde(default)]
	pub fallbacks: Vec<Target>,
}

/// TCP-specific session configuration for resolver destinations.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct TcpSession {
	pub keepalive: bool,
	pub timeout: u64,
}

/// Defines the destination for a TCP protocol rule.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TcpDestination {
	Resolver { resolver: String },
	Forward(Forward),
}

impl Validate for TcpDestination {
	fn validate(&self) -> Result<(), ValidationErrors> {
		match self {
			TcpDestination::Resolver { .. } => Ok(()),
			TcpDestination::Forward(f) => f.validate(),
		}
	}
}

/// Defines the destination for a UDP protocol rule.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UdpDestination {
	Resolver { resolver: String },
	Forward(Forward),
}

impl Validate for UdpDestination {
	fn validate(&self) -> Result<(), ValidationErrors> {
		match self {
			UdpDestination::Resolver { .. } => Ok(()),
			UdpDestination::Forward(f) => f.validate(),
		}
	}
}

// Define the static regex at the module level for the validator to use.
lazy_static::lazy_static! {
		static ref NAME_REGEX: regex::Regex = regex::Regex::new(r"^[a-z0-9]+$").unwrap();
}

/// A rule for a specific TCP protocol.
#[derive(Serialize, Deserialize, Debug, Clone, Validate, PartialEq, Eq)]
pub struct TcpProtocolRule {
	// Remove quotes from path - it should reference the static variable directly
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

/// A rule for a specific UDP protocol.
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

/// The top-level configuration structure for a TCP listener.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct TcpConfig {
	#[serde(rename = "protocols")]
	pub rules: Vec<TcpProtocolRule>,
}

// Manual implementation of Validate for TcpConfig
impl Validate for TcpConfig {
	fn validate(&self) -> Result<(), ValidationErrors> {
		let mut result = Ok(());

		// Validate each rule using nested validation
		for (i, rule) in self.rules.iter().enumerate() {
			let field_name = Box::leak(format!("rules[{}]", i).into_boxed_str());
			result = ValidationErrors::merge(result, field_name, rule.validate());
		}

		// Apply custom validation
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

/// The top-level configuration structure for a UDP listener.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct UdpConfig {
	#[serde(rename = "protocols")]
	pub rules: Vec<UdpProtocolRule>,
}

// Manual implementation of Validate for UdpConfig
impl Validate for UdpConfig {
	fn validate(&self) -> Result<(), ValidationErrors> {
		let mut result = Ok(());

		// Validate each rule using nested validation
		for (i, rule) in self.rules.iter().enumerate() {
			let field_name = Box::leak(format!("rules[{}]", i).into_boxed_str());
			result = ValidationErrors::merge(result, field_name, rule.validate());
		}

		// Apply custom validation
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

/// Custom validation for TCP rules to check for unique priorities and session logic.
pub fn validate_tcp_rules(rules: &[TcpProtocolRule]) -> Result<(), ValidationError> {
	let mut priorities = HashSet::new();
	for rule in rules {
		if !priorities.insert(rule.priority) {
			let mut err = ValidationError::new("unique_priorities");
			err.message = Some("Priorities must be unique within a listener config.".into());
			return Err(err);
		}
		if rule.session.is_some() {
			if let TcpDestination::Forward(_) = &rule.destination {
				let mut err = ValidationError::new("session_with_forward");
				err.message =
					Some("The 'session' block is only allowed for 'resolver' type destinations.".into());
				return Err(err);
			}
		}
	}
	Ok(())
}

/// Custom validation for UDP rules to check for unique priorities.
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
