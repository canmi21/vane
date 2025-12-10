/* src/modules/stack/transport/udp.rs */

use crate::modules::plugins::model::{Layer, ProcessingStep};
use serde::{Deserialize, Serialize};
use validator::{Validate, ValidationErrors};

use super::model::{Detect, Forward};

// --- Legacy `protocols` Format ---

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
	#[validate(regex(
        path = *super::model::NAME_REGEX,
        message = "can only contain lowercase letters and numbers"
    ))]
	pub name: String,
	#[validate(range(min = 1))]
	pub priority: u32,
	#[validate(nested)]
	pub detect: Detect,
	#[validate(nested)]
	pub destination: UdpDestination,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Validate)]
pub struct LegacyUdpConfig {
	#[serde(rename = "protocols")]
	#[validate(nested)]
	pub rules: Vec<UdpProtocolRule>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct FlowConfig {
	// Cannot use #[validate(nested)] on HashMap
	pub connection: ProcessingStep,
}

impl Validate for FlowConfig {
	fn validate(&self) -> Result<(), ValidationErrors> {
		// Fix: Explicitly pass Layer::L4 context for validation
		super::validator::validate_flow_config(&self.connection, Layer::L4)
	}
}

// --- Unified Configuration Enum ---

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(untagged)]
pub enum UdpConfig {
	Flow(FlowConfig),
	Legacy(LegacyUdpConfig),
}

impl Validate for UdpConfig {
	fn validate(&self) -> Result<(), ValidationErrors> {
		match self {
			UdpConfig::Legacy(config) => {
				let mut result = config.validate();
				if let Err(e) = super::validator::validate_udp_rules(&config.rules) {
					match result {
						Ok(()) => {
							let mut errors = ValidationErrors::new();
							errors.add("rules", e);
							result = Err(errors);
						}
						Err(ref mut errors) => {
							errors.add("rules", e);
						}
					}
				}
				result
			}
			UdpConfig::Flow(config) => config.validate(),
		}
	}
}
