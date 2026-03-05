// Re-export core trait and TransportContext from engine crate
pub use vane_engine::engine::context::{ExecutionContext, TransportContext};

// ApplicationContext stays here until Step 4 (vane-app extraction)
use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::RwLock;

use crate::layers::l7::container::Container;
use crate::resources::templates::{build_l7_scope, resolve_inputs};
use vane_primitives::kv::KvStore;

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
		// Temporary take ownership to wrap in Arc<RwLock> for varchain scope
		let original_container = std::mem::take(self.container);
		let container_arc = Arc::new(RwLock::new(original_container));

		// Scope must be dropped before Arc::try_unwrap to release references
		let resolved = {
			let scope = build_l7_scope(container_arc.clone());
			resolve_inputs(inputs, &scope).await
		};

		// Restore Container - scope is dropped, so try_unwrap should succeed
		match Arc::try_unwrap(container_arc) {
			Ok(container_lock) => *self.container = container_lock.into_inner(),
			Err(_) => {
				// This should never happen if scope is properly dropped
				panic!("BUG: Container Arc has lingering references after scope drop");
			}
		}

		resolved
	}

	fn as_any_mut(&mut self) -> &mut (dyn Any + Send) {
		self.container as &mut (dyn Any + Send)
	}
}
