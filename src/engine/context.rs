/* src/engine/context.rs */

use bytes::Bytes;
use std::any::Any;
use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::Value;

use crate::layers::l7::container::Container;
use crate::resources::kv::KvStore;
use crate::resources::templates::{context::L7Context, context::SimpleContext, resolve_inputs};

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

	/// Insert raw data for lazy resolution (L4/L4+ specific)
	fn insert_payload(&mut self, _key: String, _data: Bytes) {}
}

/// Transport context for L4 and L4+ layers.
pub struct TransportContext<'a> {
	pub kv: &'a mut KvStore,
	pub payloads: ahash::AHashMap<String, Bytes>,
}

#[async_trait]
impl<'a> ExecutionContext for TransportContext<'a> {
	fn kv_mut(&mut self) -> &mut KvStore {
		self.kv
	}

	async fn resolve_inputs(&mut self, inputs: &HashMap<String, Value>) -> HashMap<String, Value> {
		// Use SimpleContext with payloads support
		let mut simple_ctx = SimpleContext {
			kv: self.kv,
			payloads: Some(&self.payloads),
		};
		resolve_inputs(inputs, &mut simple_ctx).await
	}

	fn as_any_mut(&mut self) -> &mut (dyn Any + Send) {
		self.kv as &mut (dyn Any + Send)
	}

	fn insert_payload(&mut self, key: String, data: Bytes) {
		self.payloads.insert(key, data);
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
