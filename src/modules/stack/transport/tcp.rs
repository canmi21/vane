/* src/modules/stack/transport/tcp.rs */

use crate::modules::plugins::model::{Layer, ProcessingStep};
use serde::{Deserialize, Serialize};
use validator::{Validate, ValidationErrors};

use super::model::{Detect, Forward};

// --- Legacy `protocols` Format ---

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
	#[validate(regex(
        path = *super::model::NAME_REGEX,
        message = "can only contain lowercase letters and numbers"
    ))]
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

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Validate)]
pub struct LegacyTcpConfig {
	#[serde(rename = "protocols")]
	#[validate(nested)]
	pub rules: Vec<TcpProtocolRule>,
}

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
	Legacy(LegacyTcpConfig),
}

impl Validate for TcpConfig {
	fn validate(&self) -> Result<(), ValidationErrors> {
		match self {
			TcpConfig::Legacy(config) => {
				let mut result = config.validate();
				if let Err(e) = super::validator::validate_tcp_rules(&config.rules) {
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
