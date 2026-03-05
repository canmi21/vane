use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::RwLock;

use vane_engine::engine::context::ExecutionContext;
use vane_engine::templates::resolve_inputs;
use vane_primitives::kv::KvStore;

use crate::l7::container::Container;
use crate::templates::build_l7_scope;

pub struct ApplicationContext<'a> {
	pub container: &'a mut Container,
}

#[async_trait]
impl ExecutionContext for ApplicationContext<'_> {
	fn kv_mut(&mut self) -> &mut KvStore {
		&mut self.container.kv
	}

	async fn resolve_inputs(&mut self, inputs: &HashMap<String, Value>) -> HashMap<String, Value> {
		// Temporary take ownership to wrap in Arc<RwLock> for varchain scope
		let original_kv = std::mem::take(&mut self.container.kv);
		let original_headers = std::mem::take(&mut self.container.request_headers);
		let original_resp_headers = std::mem::take(&mut self.container.response_headers);

		let temp_container = Container::new(
			original_kv,
			original_headers,
			crate::l7::container::PayloadState::Empty,
			original_resp_headers,
			crate::l7::container::PayloadState::Empty,
			None,
		);
		let container_arc = Arc::new(RwLock::new(temp_container));

		// Scope must be dropped before Arc::try_unwrap
		let resolved = {
			let scope = build_l7_scope(container_arc.clone());
			resolve_inputs(inputs, &scope).await
		};

		// Restore container fields
		match Arc::try_unwrap(container_arc) {
			Ok(lock) => {
				let temp = lock.into_inner();
				self.container.kv = temp.kv;
				self.container.request_headers = temp.request_headers;
				self.container.response_headers = temp.response_headers;
			}
			Err(_) => {
				panic!("BUG: Container Arc has lingering references after scope drop");
			}
		}

		resolved
	}

	fn as_any_mut(&mut self) -> &mut (dyn std::any::Any + Send) {
		self.container
	}
}
