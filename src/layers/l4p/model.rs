/* src/layers/l4p/model.rs */

use crate::engine::interfaces::{Layer, ProcessingStep};
use serde::{Deserialize, Serialize};
#[cfg(feature = "console")]
use utoipa::ToSchema;
use validator::{Validate, ValidationErrors};

/// Represents the configuration for a specific L4+ protocol resolver (e.g., "tls", "http").
///
/// Unlike L4 listeners which are bound to ports, resolvers are bound to protocol names.
/// They define the flow logic that handles a connection *after* it has been upgraded.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, ToSchema)]
pub struct ResolverConfig {
	// The main flow logic for this protocol
	pub connection: ProcessingStep,
	#[serde(skip)]
	#[schema(ignore)]
	pub protocol: String,
}

impl Validate for ResolverConfig {
	fn validate(&self) -> Result<(), ValidationErrors> {
		// Validation must be context-aware.
		if self.protocol.is_empty() {
			return Ok(());
		}
		// We validate as L4Plus to allow Upgraders but restrict L7-only components if any.
		// Terminators check this layer context.
		use crate::layers::l4::validator;
		validator::validate_flow_config(&self.connection, Layer::L4Plus, &self.protocol)
	}
}

/// Defines the hardcoded list of supported L4 -> L4+ upgrade protocols.
pub const SUPPORTED_UPGRADE_PROTOCOLS: &[&str] = &["tls", "http", "quic"];
