/* src/layers/l7/model.rs */

use crate::engine::interfaces::{Layer, ProcessingStep};
use serde::{Deserialize, Serialize};
#[cfg(feature = "console")]
use utoipa::ToSchema;
use validator::{Validate, ValidationErrors};

/// Represents the configuration for a specific L7 application protocol (e.g., "httpx").
///
/// L7 protocols handle the request/response lifecycle after TLS/QUIC termination.
/// The `pipeline` defines the middleware chain (Request -> Upstream -> Response).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, ToSchema)]
pub struct ApplicationConfig {
	// The middleware pipeline for this protocol.
	// In Vane's L7 model, "Fetch Upstream" is just another middleware in this chain.
	pub pipeline: ProcessingStep,
	#[serde(skip)]
	#[schema(ignore)]
	pub protocol: String,
}

impl Validate for ApplicationConfig {
	fn validate(&self) -> Result<(), ValidationErrors> {
		if self.protocol.is_empty() {
			return Ok(());
		}
		use crate::layers::l4::validator;
		// Validate with L7 context to enable HTTP-specific plugin checks
		validator::validate_flow_config(&self.pipeline, Layer::L7, &self.protocol)
	}
}

/// Defines the hardcoded list of supported L7 protocols.
pub const SUPPORTED_APP_PROTOCOLS: &[&str] = &["httpx"];
