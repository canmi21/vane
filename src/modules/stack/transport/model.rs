/* src/modules/stack/transport/model.rs */

use serde::{Deserialize, Serialize};
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

lazy_static::lazy_static! {
	pub(super) static ref NAME_REGEX: regex::Regex = regex::Regex::new(r"^[a-z0-9]+$").unwrap();
}
