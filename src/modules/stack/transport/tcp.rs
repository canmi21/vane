/* src/modules/stack/transport/tcp.rs */

use serde::{Deserialize, Serialize};
use validator::{Validate, ValidationErrors};

use super::model::{Detect, Forward, NAME_REGEX};

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
		if let Err(e) = super::validator::validate_tcp_rules(&self.rules) {
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
