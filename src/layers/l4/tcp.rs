/* src/layers/l4/tcp.rs */

use crate::engine::interfaces::{Layer, ProcessingStep};
use serde::{Deserialize, Serialize};
use validator::{Validate, ValidationErrors};

use super::legacy;

// --- New `connection` (Flow) Format ---

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct FlowConfig {
	// Cannot use #[validate(nested)] on HashMap
	pub connection: ProcessingStep,
}

impl Validate for FlowConfig {
	fn validate(&self) -> Result<(), ValidationErrors> {
		// Fix: Explicitly pass Layer::L4 context for validation
		super::validator::validate_flow_config(&self.connection, Layer::L4, "tcp")
	}
}

// --- Unified Configuration Enum ---

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(untagged)]
pub enum TcpConfig {
	Flow(FlowConfig),
	Legacy(legacy::LegacyTcpConfig),
}

impl Validate for TcpConfig {
	fn validate(&self) -> Result<(), ValidationErrors> {
		match self {
			TcpConfig::Legacy(config) => {
				let mut result = config.validate();
				if let Err(e) = legacy::validate_tcp_rules(&config.rules) {
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
			TcpConfig::Flow(config) => config.validate(),
		}
	}
}
