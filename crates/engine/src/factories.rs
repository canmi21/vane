use std::collections::HashMap;
use std::sync::Arc;

use vane_core::{FetchKind, MiddlewareKind};

use crate::flow_graph::{FetchInst, MiddlewareInst};

pub type MiddlewareFactoryFn =
	dyn Fn(&serde_json::Value) -> Result<MiddlewareInst, FactoryError> + Send + Sync;

pub type FetchFactoryFn =
	dyn Fn(&serde_json::Value) -> Result<FetchInst, FactoryError> + Send + Sync;

/// Structured factory rejection.
///
/// The prior shape was a single-field tuple struct that stringified
/// every error class into one bucket; downstream observers
/// (link-pass error log, `compile_dry_run` mgmt verb) lost the source
/// chain. The enum now carries:
///
/// * [`FactoryError::Invalid`] — generic semantic rejection with an
///   operator-facing message (e.g. "version 'h3' requires args.tls").
/// * [`FactoryError::InvalidArgs`] — `serde_json` shape mismatch
///   pinned to a factory name; the underlying `serde_json::Error`
///   travels as `#[source]` so the link pass's tracing can render
///   the parse location.
/// * [`FactoryError::Unknown`] — factory name not registered.
/// * [`FactoryError::Inner`] — `vane_core::Error` from a fallible
///   sub-operation (CRL source parse, TLS cfg build, ...). Surfaces
///   the full source chain by `#[error(transparent)]`.
///
/// `#[non_exhaustive]` so future classes can land without breaking
/// downstream match sites.
#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum FactoryError {
	#[error("{0}")]
	Invalid(String),
	#[error("invalid args for {name}: {source}")]
	InvalidArgs {
		name: String,
		#[source]
		source: serde_json::Error,
	},
	#[error("unknown factory: {0}")]
	Unknown(String),
	#[error(transparent)]
	Inner(#[from] vane_core::Error),
}

impl FactoryError {
	/// Operator-facing message extractor — primarily for tests that
	/// pattern-match `Err(FactoryError::Invalid(msg))`. Returns the
	/// message for `Invalid` / `Unknown`, the `Display` of the
	/// inner error for `Inner`, and a synthesised string for
	/// `InvalidArgs`.
	#[must_use]
	pub fn message(&self) -> std::borrow::Cow<'_, str> {
		match self {
			Self::Invalid(s) | Self::Unknown(s) => std::borrow::Cow::Borrowed(s.as_str()),
			Self::InvalidArgs { name, source } => {
				std::borrow::Cow::Owned(format!("invalid args for {name}: {source}"))
			}
			Self::Inner(e) => std::borrow::Cow::Owned(e.to_string()),
		}
	}
}

pub enum MiddlewareFactoryEntry {
	Available {
		/// The [`MiddlewareKind`] the factory claims to produce. The link pass
		/// cross-checks this against `SymbolicMiddlewareRef::kind` and against
		/// the actual `MiddlewareInst` variant the factory returned so a
		/// registry wiring mistake fails fast with a pointed error instead of
		/// a wrong-phase runtime panic.
		kind: MiddlewareKind,
		construct: Box<MiddlewareFactoryFn>,
	},
	FeatureGated(&'static str),
}

pub enum FetchFactoryEntry {
	Available(Box<FetchFactoryFn>),
	FeatureGated(&'static str),
}

#[derive(Default)]
pub struct MiddlewareFactories {
	inner: HashMap<Arc<str>, MiddlewareFactoryEntry>,
}

impl MiddlewareFactories {
	#[must_use]
	pub fn new() -> Self {
		Self { inner: HashMap::new() }
	}

	pub fn register<F>(&mut self, name: &str, kind: MiddlewareKind, construct: F)
	where
		F: Fn(&serde_json::Value) -> Result<MiddlewareInst, FactoryError> + Send + Sync + 'static,
	{
		self.inner.insert(
			Arc::from(name),
			MiddlewareFactoryEntry::Available { kind, construct: Box::new(construct) },
		);
	}

	pub fn register_feature_gated(&mut self, name: &str, feature: &'static str) {
		self.inner.insert(Arc::from(name), MiddlewareFactoryEntry::FeatureGated(feature));
	}

	#[must_use]
	pub fn get(&self, name: &str) -> Option<&MiddlewareFactoryEntry> {
		self.inner.get(name)
	}
}

#[derive(Default)]
pub struct FetchFactories {
	inner: HashMap<FetchKind, FetchFactoryEntry>,
}

impl FetchFactories {
	#[must_use]
	pub fn new() -> Self {
		Self { inner: HashMap::new() }
	}

	pub fn register<F>(&mut self, kind: FetchKind, construct: F)
	where
		F: Fn(&serde_json::Value) -> Result<FetchInst, FactoryError> + Send + Sync + 'static,
	{
		self.inner.insert(kind, FetchFactoryEntry::Available(Box::new(construct)));
	}

	pub fn register_feature_gated(&mut self, kind: FetchKind, feature: &'static str) {
		self.inner.insert(kind, FetchFactoryEntry::FeatureGated(feature));
	}

	#[must_use]
	pub fn get(&self, kind: FetchKind) -> Option<&FetchFactoryEntry> {
		self.inner.get(&kind)
	}
}
