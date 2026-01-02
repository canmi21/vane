/* src/modules/stack/application/model.rs */

use crate::modules::{
	plugins::core::model::{Layer, ProcessingStep},
	stack::transport::loader::PreProcess,
};
use arc_swap::ArcSwap;
use dashmap::DashMap;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use validator::{Validate, ValidationErrors};

/// Represents the configuration for a specific L7 application protocol (e.g., "httpx").
///
/// L7 protocols handle the request/response lifecycle after TLS/QUIC termination.
/// The `pipeline` defines the middleware chain (Request -> Upstream -> Response).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ApplicationConfig {
	// The middleware pipeline for this protocol.
	// In Vane's L7 model, "Fetch Upstream" is just another middleware in this chain.
	pub pipeline: ProcessingStep,
	#[serde(skip)]
	pub protocol: String,
}

impl Validate for ApplicationConfig {
	fn validate(&self) -> Result<(), ValidationErrors> {
		use crate::modules::stack::transport::validator;
		// Validate with L7 context to enable HTTP-specific plugin checks
		validator::validate_flow_config(&self.pipeline, Layer::L7, &self.protocol)
	}
}

impl PreProcess for ApplicationConfig {
	fn pre_process(&mut self) {
		// Future proofing: Lowercase keys or normalize inputs if needed.
	}

	fn set_context(&mut self, context: &str) {
		self.protocol = context.to_string();
	}
}

/// A global, thread-safe registry of active application configurations.
/// Key: Protocol Name (e.g., "httpx")
/// Value: The parsed configuration
pub static APPLICATION_REGISTRY: Lazy<ArcSwap<DashMap<String, Arc<ApplicationConfig>>>> =
	Lazy::new(|| ArcSwap::new(Arc::new(DashMap::new())));

/// Defines the hardcoded list of supported L7 protocols.
pub const SUPPORTED_APP_PROTOCOLS: &[&str] = &["httpx"];
