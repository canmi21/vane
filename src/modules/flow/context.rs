/* src/modules/flow/context.rs */

use std::any::Any;
use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::Value;

use crate::modules::kv::KvStore;
use crate::modules::stack::application::container::Container;
use crate::modules::template::{context::L7Context, context::SimpleContext, resolve_inputs};

/// Execution context abstraction for flow engine.
///
/// Different layers provide different contexts:
/// - L4/L4+: KV Store only (TransportContext)
/// - L7: Container with headers/body (ApplicationContext)
#[async_trait]
pub trait ExecutionContext: Send {
	/// Get mutable reference to KV store (all layers have this)
	fn kv_mut(&mut self) -> &mut KvStore;

	/// Resolve template inputs using layer-specific logic
	///
	/// L4/L4+: SimpleContext (KV lookup only)
	/// L7: L7Context (hijacking support for {{req.body}}, {{res.header.*}}, etc.)
	async fn resolve_inputs(&mut self, inputs: &HashMap<String, Value>) -> HashMap<String, Value>;

	/// Get type-erased context for plugins that need it
	///
	/// Some terminators need access to ConnectionObject or Container.
	/// This provides the underlying context as `&mut (dyn Any + Send)`.
	fn as_any_mut(&mut self) -> &mut (dyn Any + Send);
}

/// Transport context for L4 and L4+ layers.
///
/// Only has KV store, no protocol-specific data.
pub struct TransportContext<'a> {
	pub kv: &'a mut KvStore,
}

#[async_trait]
impl<'a> ExecutionContext for TransportContext<'a> {
	fn kv_mut(&mut self) -> &mut KvStore {
		self.kv
	}

	async fn resolve_inputs(&mut self, inputs: &HashMap<String, Value>) -> HashMap<String, Value> {
		// Use SimpleContext (KV lookup only, no hijacking)
		let mut simple_ctx = SimpleContext { kv: self.kv };
		resolve_inputs(inputs, &mut simple_ctx).await
	}

	fn as_any_mut(&mut self) -> &mut (dyn Any + Send) {
		self.kv as &mut (dyn Any + Send)
	}
}

/// Application context for L7 layer.
///
/// Contains full Container with headers, body, protocol data.
pub struct ApplicationContext<'a> {
	pub container: &'a mut Container,
}

#[async_trait]
impl<'a> ExecutionContext for ApplicationContext<'a> {
	fn kv_mut(&mut self) -> &mut KvStore {
		&mut self.container.kv
	}

	async fn resolve_inputs(&mut self, inputs: &HashMap<String, Value>) -> HashMap<String, Value> {
		// Use L7Context (supports hijacking)
		let mut l7_ctx = L7Context {
			container: self.container,
		};
		resolve_inputs(inputs, &mut l7_ctx).await
	}

	fn as_any_mut(&mut self) -> &mut (dyn Any + Send) {
		self.container as &mut (dyn Any + Send)
	}
}
