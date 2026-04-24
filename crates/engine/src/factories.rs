use std::collections::HashMap;
use std::sync::Arc;

use vane_core::{FetchKind, MiddlewareKind};

use crate::flow_graph::{FetchInst, MiddlewareInst};

pub type MiddlewareFactoryFn =
	dyn Fn(&serde_json::Value) -> Result<MiddlewareInst, FactoryError> + Send + Sync;

pub type FetchFactoryFn =
	dyn Fn(&serde_json::Value) -> Result<FetchInst, FactoryError> + Send + Sync;

#[derive(thiserror::Error, Debug)]
#[error("factory rejected args: {0}")]
pub struct FactoryError(pub String);

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
