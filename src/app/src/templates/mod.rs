/* src/app/src/templates/mod.rs */

pub mod source;

pub use source::http::HttpSource;

use std::sync::Arc;
use tokio::sync::RwLock;
use varchain::{Scope, Source, SourceFuture};

use crate::l7::container::Container;

/// Helper struct for Container KV source with Arc
struct AsyncContainerKvSource {
	container: Arc<RwLock<Container>>,
}

impl Source for AsyncContainerKvSource {
	fn get(&self, key: &str) -> SourceFuture<'_, String> {
		let key = key.to_owned();
		let container = self.container.clone();
		Box::pin(async move { container.read().await.kv.get(&key).cloned().into() })
	}
}

/// Build scope for L7 HTTP
pub fn build_l7_scope(container: Arc<RwLock<Container>>) -> Scope {
	Scope::new()
		.push(HttpSource { container: container.clone() })
		.push(AsyncContainerKvSource { container })
}
