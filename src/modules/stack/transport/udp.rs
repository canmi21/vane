/* src/modules/stack/transport/udp.rs */

use serde::{Deserialize, Serialize};
use validator::{Validate, ValidationErrors};

use super::model::{Detect, Forward, NAME_REGEX};

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
		if let Err(e) = super::validator::validate_udp_rules(&self.rules) {
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
