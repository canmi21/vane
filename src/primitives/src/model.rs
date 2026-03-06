/* src/primitives/src/model.rs */

use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::net::IpAddr;
use validator::{Validate, ValidationError, ValidationErrors, ValidationErrorsKind};

/// Validation error for flow configuration, carrying a path and message.
#[derive(Debug)]
pub struct FlowValidationError {
	pub path: String,
	pub message: String,
}

#[must_use]
pub fn validate_target(target: &Target, path: &str) -> Vec<FlowValidationError> {
	let mut errors = Vec::new();
	if let Target::Domain { domain, .. } = target
		&& !cfg!(feature = "domain-target")
	{
		errors.push(FlowValidationError {
			path: path.to_owned(),
			message: format!(
				"Domain target '{domain}' is disabled in this build. Please recompile with 'domain-target' feature enabled."
			),
		});
	}
	errors
}

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
			Self::Ip { ip, port } => {
				if ip.parse::<IpAddr>().is_err() {
					errors.add("ip", ValidationError::new("ip"));
				}
				if *port == 0 {
					let mut err = ValidationError::new("range");
					err.message = Some("port must be greater than 0".into());
					errors.add("port", err);
				}
			}
			Self::Domain { domain, port } => {
				if domain.is_empty() || domain.len() > 253 {
					errors.add("domain", ValidationError::new("hostname"));
				}
				if *port == 0 {
					let mut err = ValidationError::new("range");
					err.message = Some("port must be greater than 0".into());
					errors.add("port", err);
				}
			}
			Self::Node { port, .. } => {
				if *port == 0 {
					let mut err = ValidationError::new("range");
					err.message = Some("port must be greater than 0".into());
					errors.add("port", err);
				}
			}
		}
		if errors.is_empty() { Ok(()) } else { Err(errors) }
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
			let path = format!("targets[{i}]");
			let target_errors_list = validate_target(target, &path);
			for flow_err in target_errors_list {
				let mut err = ValidationError::new("feature_disabled");
				err.message = Some(flow_err.message.into());
				errors.add("targets", err);
			}

			if let Err(target_errors) = target.validate() {
				for (field, kind) in target_errors.errors() {
					if let ValidationErrorsKind::Field(field_errors) = kind {
						for error in field_errors {
							let mut err = error.clone();
							let old_msg = err.message.clone().unwrap_or_else(|| Cow::from("invalid"));
							err.message = Some(format!("[index {i}] {field}: {old_msg}").into());
							errors.add("targets", err);
						}
					}
				}
			}
		}

		for (i, target) in self.fallbacks.iter().enumerate() {
			let path = format!("fallbacks[{i}]");
			let target_errors_list = validate_target(target, &path);
			for flow_err in target_errors_list {
				let mut err = ValidationError::new("feature_disabled");
				err.message = Some(flow_err.message.into());
				errors.add("fallbacks", err);
			}

			if let Err(target_errors) = target.validate() {
				for (field, kind) in target_errors.errors() {
					if let ValidationErrorsKind::Field(field_errors) = kind {
						for error in field_errors {
							let mut err = error.clone();
							let old_msg = err.message.clone().unwrap_or_else(|| Cow::from("invalid"));
							err.message = Some(format!("[index {i}] {field}: {old_msg}").into());
							errors.add("fallbacks", err);
						}
					}
				}
			}
		}

		if errors.is_empty() { Ok(()) } else { Err(errors) }
	}
}

pub static NAME_REGEX: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
	regex::Regex::new(r"^[a-z0-9_-]+$").expect("Failed to compile NAME_REGEX")
});
