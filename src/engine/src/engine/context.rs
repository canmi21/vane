use bytes::Bytes;
use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::RwLock;

use crate::templates::{build_l4_scope, resolve_inputs};
use vane_primitives::kv::KvStore;

/// Execution context abstraction for flow engine.
///
/// Different layers provide different contexts:
/// - L4/L4+: KV Store only (TransportContext)
/// - L7: Container with headers/body (ApplicationContext — in vane-app)
#[async_trait]
pub trait ExecutionContext: Send {
	/// Get mutable reference to KV store (all layers have this)
	fn kv_mut(&mut self) -> &mut KvStore;

	/// Resolve template inputs using layer-specific logic
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
		// Temporary take ownership to wrap in Arc<RwLock> for varchain scope
		let original_kv = std::mem::take(self.kv);
		let kv_arc = Arc::new(RwLock::new(original_kv));
		let payloads_arc = Arc::new(self.payloads.clone());

		// Scope must be dropped before Arc::try_unwrap to release references
		let resolved = {
			let scope = build_l4_scope(kv_arc.clone(), Some(payloads_arc));
			resolve_inputs(inputs, &scope).await
		};

		// Restore KV - scope is dropped, so try_unwrap should succeed
		match Arc::try_unwrap(kv_arc) {
			Ok(kv_lock) => *self.kv = kv_lock.into_inner(),
			Err(_) => {
				// This should never happen if scope is properly dropped
				panic!("BUG: KV Arc has lingering references after scope drop");
			}
		}

		resolved
	}

	fn as_any_mut(&mut self) -> &mut (dyn Any + Send) {
		self.kv as &mut (dyn Any + Send)
	}

	fn insert_payload(&mut self, key: String, data: Bytes) {
		self.payloads.insert(key, data);
	}
}
