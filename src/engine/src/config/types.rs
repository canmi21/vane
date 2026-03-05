/* src/engine/src/config/types.rs */

use crate::engine::interfaces::{Layer, ProcessingStep};
use live::loader::PreProcess;
use serde::{Deserialize, Serialize};
#[cfg(feature = "console")]
use utoipa::ToSchema;
use validator::{Validate, ValidationError, ValidationErrors};

pub use vane_primitives::certs::arcswap::LoadedCert as CertEntry;
pub use vane_primitives::model::{Detect, DetectMethod, Forward, Strategy};
pub use vane_primitives::service_discovery::model::NodesConfig;

// ---------------------------------------------------------------------------
// Legacy TCP types (pure serde structs — dispatch functions stay in transport)
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct TcpSession {
	pub keepalive: bool,
	pub timeout: u64,
}

impl Validate for TcpSession {
	fn validate(&self) -> Result<(), ValidationErrors> {
		if self.timeout == 0 {
			let mut errors = ValidationErrors::new();
			let mut err = ValidationError::new("range");
			err.message = Some("timeout must be greater than 0".into());
			errors.add("timeout", err);
			return Err(errors);
		}
		Ok(())
	}
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
			Self::Resolver { .. } => Ok(()),
			Self::Forward { forward } => forward.validate(),
		}
	}
}

#[derive(Serialize, Deserialize, Debug, Clone, Validate, PartialEq, Eq)]
pub struct TcpProtocolRule {
	#[validate(regex(
        path = *vane_primitives::model::NAME_REGEX,
        message = "can only contain lowercase letters, numbers, underscores and hyphens"
    ))]
	pub name: String,
	#[validate(range(min = 1))]
	pub priority: u32,
	#[validate(nested)]
	pub detect: Detect,
	#[serde(default)]
	#[validate(nested)]
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

// ---------------------------------------------------------------------------
// Legacy UDP types
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UdpDestination {
	Resolver { resolver: String },
	Forward { forward: Forward },
}

impl Validate for UdpDestination {
	fn validate(&self) -> Result<(), ValidationErrors> {
		match self {
			Self::Resolver { .. } => Ok(()),
			Self::Forward { forward } => forward.validate(),
		}
	}
}

#[derive(Serialize, Deserialize, Debug, Clone, Validate, PartialEq, Eq)]
pub struct UdpProtocolRule {
	#[validate(regex(
        path = *vane_primitives::model::NAME_REGEX,
        message = "can only contain lowercase letters, numbers, underscores and hyphens"
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

// ---------------------------------------------------------------------------
// Flow-based listener configs (L4)
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct TcpFlowConfig {
	pub connection: ProcessingStep,
}

impl Validate for TcpFlowConfig {
	fn validate(&self) -> Result<(), ValidationErrors> {
		crate::shared::validator::validate_flow_config(&self.connection, Layer::L4, "tcp")
	}
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(untagged)]
pub enum TcpConfig {
	Flow(TcpFlowConfig),
	Legacy(LegacyTcpConfig),
}

impl Validate for TcpConfig {
	fn validate(&self) -> Result<(), ValidationErrors> {
		match self {
			Self::Legacy(config) => {
				let mut result = config.validate();
				if let Err(e) = validate_tcp_rules(&config.rules) {
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
			Self::Flow(config) => config.validate(),
		}
	}
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct UdpFlowConfig {
	pub connection: ProcessingStep,
}

impl Validate for UdpFlowConfig {
	fn validate(&self) -> Result<(), ValidationErrors> {
		crate::shared::validator::validate_flow_config(&self.connection, Layer::L4, "udp")
	}
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(untagged)]
pub enum UdpConfig {
	Flow(UdpFlowConfig),
	Legacy(LegacyUdpConfig),
}

impl Validate for UdpConfig {
	fn validate(&self) -> Result<(), ValidationErrors> {
		match self {
			Self::Legacy(config) => {
				let mut result = config.validate();
				if let Err(e) = validate_udp_rules(&config.rules) {
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
			Self::Flow(config) => config.validate(),
		}
	}
}

// ---------------------------------------------------------------------------
// L4+ Resolver config
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "console", derive(ToSchema))]
pub struct ResolverConfig {
	pub connection: ProcessingStep,
	#[serde(skip)]
	#[cfg_attr(feature = "console", schema(ignore))]
	pub protocol: String,
}

impl Validate for ResolverConfig {
	fn validate(&self) -> Result<(), ValidationErrors> {
		if self.protocol.is_empty() {
			return Ok(());
		}
		crate::shared::validator::validate_flow_config(&self.connection, Layer::L4Plus, &self.protocol)
	}
}

/// Hardcoded list of supported L4 -> L4+ upgrade protocols.
pub const SUPPORTED_UPGRADE_PROTOCOLS: &[&str] = &["tls", "http", "quic"];

// ---------------------------------------------------------------------------
// L7 Application config
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "console", derive(ToSchema))]
pub struct ApplicationConfig {
	pub pipeline: ProcessingStep,
	#[serde(skip)]
	#[cfg_attr(feature = "console", schema(ignore))]
	pub protocol: String,
}

impl Validate for ApplicationConfig {
	fn validate(&self) -> Result<(), ValidationErrors> {
		if self.protocol.is_empty() {
			return Ok(());
		}
		crate::shared::validator::validate_flow_config(&self.pipeline, Layer::L7, &self.protocol)
	}
}

/// Hardcoded list of supported L7 protocols.
pub const SUPPORTED_APP_PROTOCOLS: &[&str] = &["httpx"];

// ---------------------------------------------------------------------------
// LazyCert config (simple serde struct, implementation stays in binary crate)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct LazyCertConfig {
	/// Enable LazyCert integration
	#[serde(default)]
	pub enabled: bool,

	/// LazyCert API URL
	#[validate(url)]
	pub url: String,

	/// API access token
	#[validate(length(min = 1, message = "token cannot be empty"))]
	pub token: String,

	/// Challenge poll interval in seconds
	#[serde(default = "default_poll_interval")]
	#[validate(range(min = 1, max = 300))]
	pub poll_interval: u64,

	/// Self-reported public IP ("auto" or explicit IP)
	#[serde(default = "default_public_ip")]
	pub public_ip: String,
}

fn default_poll_interval() -> u64 {
	5
}

fn default_public_ip() -> String {
	"auto".to_owned()
}

// ---------------------------------------------------------------------------
// Standalone legacy validation helpers
// ---------------------------------------------------------------------------

pub fn validate_tcp_rules(rules: &[TcpProtocolRule]) -> Result<(), ValidationError> {
	let mut priorities = std::collections::HashSet::new();
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
	let mut priorities = std::collections::HashSet::new();
	for rule in rules {
		if !priorities.insert(rule.priority) {
			let mut err = ValidationError::new("unique_priorities");
			err.message = Some("Priorities must be unique within a listener config.".into());
			return Err(err);
		}
	}
	Ok(())
}

// ---------------------------------------------------------------------------
// PreProcess impls (for live crate config hot-reload)
// ---------------------------------------------------------------------------

impl PreProcess for TcpConfig {
	fn pre_process(&mut self) {
		if let Self::Legacy(config) = self {
			for rule in &mut config.rules {
				rule.name = rule.name.to_lowercase();
			}
		}
	}
}

impl PreProcess for UdpConfig {
	fn pre_process(&mut self) {
		if let Self::Legacy(config) = self {
			for rule in &mut config.rules {
				rule.name = rule.name.to_lowercase();
			}
		}
	}
}

impl PreProcess for LazyCertConfig {
	fn pre_process(&mut self) {
		self.url = self.url.trim_end_matches('/').to_owned();
	}
}

impl PreProcess for ResolverConfig {
	fn set_context(&mut self, ctx: &str) {
		self.protocol = ctx.to_owned();
	}
}

impl PreProcess for ApplicationConfig {
	fn set_context(&mut self, ctx: &str) {
		self.protocol = ctx.to_owned();
	}
}
