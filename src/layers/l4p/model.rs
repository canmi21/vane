/* src/layers/l4p/model.rs */

use crate::engine::contract::{Layer, ProcessingStep};
use crate::layers::l4::loader::PreProcess;
use arc_swap::ArcSwap;
use dashmap::DashMap;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use validator::{Validate, ValidationErrors};

/// Represents the configuration for a specific L4+ protocol resolver (e.g., "tls", "http").
///
/// Unlike L4 listeners which are bound to ports, resolvers are bound to protocol names.
/// They define the flow logic that handles a connection *after* it has been upgraded.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ResolverConfig {
	// The main flow logic for this protocol
	pub connection: ProcessingStep,
	#[serde(skip)]
	pub protocol: String,
}

impl Validate for ResolverConfig {
	fn validate(&self) -> Result<(), ValidationErrors> {
		// Validation must be context-aware.
		// We validate as L4Plus to allow Upgraders but restrict L7-only components if any.
		// Terminators check this layer context.
		use crate::layers::l4::validator;
		validator::validate_flow_config(&self.connection, Layer::L4Plus, &self.protocol)
	}
}

// Implement PreProcess trait required by the loader.
// Currently, ResolverConfig needs no special pre-processing (like lowercasing names).
impl PreProcess for ResolverConfig {
	fn pre_process(&mut self) {
		// No-op
	}

	fn set_context(&mut self, context: &str) {
		self.protocol = context.to_string();
	}
}

/// A global, thread-safe registry of active resolver configurations.
/// Key: Protocol Name (e.g., "tls", "http", "quic")
/// Value: The parsed configuration
pub static RESOLVER_REGISTRY: Lazy<ArcSwap<DashMap<String, Arc<ResolverConfig>>>> =
	Lazy::new(|| ArcSwap::new(Arc::new(DashMap::new())));

/// Defines the hardcoded list of supported L4 -> L4+ upgrade protocols.
pub const SUPPORTED_UPGRADE_PROTOCOLS: &[&str] = &["tls", "http", "quic"];
