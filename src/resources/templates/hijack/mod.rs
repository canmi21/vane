/* src/resources/templates/hijack/mod.rs */

use anyhow::Result;
use async_trait::async_trait;

pub mod l4p;
pub mod l7_http;

/// Hijacker trait for layer-specific keyword handling
#[async_trait]
pub trait Hijacker: Send + Sync {
	/// Check if this hijacker handles the given key
	fn can_handle(&self, key: &str) -> bool;

	/// Resolve the hijack keyword
	async fn resolve(&mut self, key: &str) -> Result<String>;
}
