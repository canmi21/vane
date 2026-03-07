use std::collections::HashMap;

use super::plugin::PluginAction;

/// Holds all registered plugins by name.
///
/// ```
/// use vane_engine::flow::{BranchAction, ExecutionContext, Middleware, PluginAction, PluginRegistry};
///
/// struct Noop;
/// impl Middleware for Noop {
///     fn execute(
///         &self,
///         _params: &serde_json::Value,
///         _ctx: &dyn ExecutionContext,
///     ) -> Result<BranchAction, anyhow::Error> {
///         Ok(BranchAction { branch: "ok".to_owned(), updates: vec![] })
///     }
/// }
///
/// let registry = PluginRegistry::new()
///     .register("noop", PluginAction::Middleware(Box::new(Noop)));
///
/// assert!(registry.get("noop").is_some());
/// assert!(registry.get("missing").is_none());
/// ```
pub struct PluginRegistry {
	plugins: HashMap<String, PluginAction>,
}

impl Default for PluginRegistry {
	fn default() -> Self {
		Self::new()
	}
}

impl PluginRegistry {
	pub fn new() -> Self {
		Self { plugins: HashMap::new() }
	}

	#[must_use]
	pub fn register(mut self, name: impl Into<String>, action: PluginAction) -> Self {
		self.plugins.insert(name.into(), action);
		self
	}

	pub fn get(&self, name: &str) -> Option<&PluginAction> {
		self.plugins.get(name)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::flow::context::ExecutionContext;
	use crate::flow::plugin::{BranchAction, Middleware};

	struct DummyMiddleware;
	impl Middleware for DummyMiddleware {
		fn execute(
			&self,
			_params: &serde_json::Value,
			_ctx: &dyn ExecutionContext,
		) -> Result<BranchAction, anyhow::Error> {
			Ok(BranchAction { branch: "ok".to_owned(), updates: vec![] })
		}
	}

	#[test]
	fn register_and_get() {
		let registry =
			PluginRegistry::new().register("test", PluginAction::Middleware(Box::new(DummyMiddleware)));
		assert!(registry.get("test").is_some());
	}

	#[test]
	fn get_missing_returns_none() {
		let registry = PluginRegistry::new();
		assert!(registry.get("nonexistent").is_none());
	}
}
