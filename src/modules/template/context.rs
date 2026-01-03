/* src/modules/template/context.rs */

use async_trait::async_trait;
use bytes::Bytes;
use fancy_log::{LogLevel, log};
use std::collections::HashMap;

use crate::modules::kv::KvStore;
use crate::modules::stack::application::container::Container;

use super::hijack::{self, Hijacker};

/// Template resolution context
#[async_trait]
pub trait TemplateContext: Send {
	/// Resolve a single key to string value
	/// Returns original template string ({{key}}) if not found
	async fn get(&mut self, key: &str) -> String;
}

/// L4/L4+ simple context (KV Store only, but supports hijacking for raw payloads)
pub struct SimpleContext<'a> {
	pub kv: &'a mut KvStore,
	pub payloads: Option<&'a HashMap<String, Bytes>>,
}

#[async_trait]
impl<'a> TemplateContext for SimpleContext<'a> {
	async fn get(&mut self, key: &str) -> String {
		// 1. Try hijacking first if raw payloads are provided
		if let Some(payloads) = self.payloads {
			let mut hijacker = hijack::l4p::L4PlusHijacker {
				kv: self.kv,
				payloads,
			};

			if hijacker.can_handle(key) {
				match hijacker.resolve(key).await {
					Ok(value) => return value,
					Err(e) => {
						log(
							LogLevel::Warn,
							&format!(
								"⚠ L4+ Hijacking failed for '{}': {}, trying KV fallback",
								key, e
							),
						);
					}
				}
			}
		}

		// 2. Standard KV lookup
		match self.kv.get(key) {
			Some(value) => value.clone(),
			None => {
				log(
					LogLevel::Warn,
					&format!(
						"⚠ Template key '{}' not found in KV Store, keeping original: {{{{{}}}}}",
						key, key
					),
				);
				format!("{{{{{}}}}}", key) // Return original {{key}}
			}
		}
	}
}

/// L7 context with hijacking support
pub struct L7Context<'a> {
	pub container: &'a mut Container,
}

#[async_trait]
impl<'a> TemplateContext for L7Context<'a> {
	async fn get(&mut self, key: &str) -> String {
		// 1. Try hijacking first (layer + protocol specific)
		let mut hijacker = hijack::l7_http::HttpHijacker {
			container: self.container,
		};

		if hijacker.can_handle(key) {
			match hijacker.resolve(key).await {
				Ok(value) => return value,
				Err(e) => {
					log(
						LogLevel::Warn,
						&format!(
							"⚠ Hijacking failed for '{}': {}, trying KV fallback",
							key, e
						),
					);
					// Fall through to KV Store
				}
			}
		}

		// 2. Fallback to KV Store
		match self.container.kv.get(key) {
			Some(value) => value.clone(),
			None => {
				log(
					LogLevel::Warn,
					&format!(
						"⚠ Template key '{}' not found, keeping original: {{{{{}}}}}",
						key, key
					),
				);
				format!("{{{{{}}}}}", key) // Return original {{key}}
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Tests SimpleContext returns value from KV Store.
	#[tokio::test]
	async fn test_simple_context_found() {
		let mut kv = KvStore::new();
		kv.insert("key".to_string(), "value".to_string());

		let mut context = SimpleContext {
			kv: &mut kv,
			payloads: None,
		};
		let result = context.get("key").await;

		assert_eq!(result, "value");
	}

	/// Tests SimpleContext returns original template when key not found.
	#[tokio::test]
	async fn test_simple_context_not_found() {
		let mut kv = KvStore::new();
		let mut context = SimpleContext {
			kv: &mut kv,
			payloads: None,
		};
		let result = context.get("missing").await;

		assert_eq!(result, "{{missing}}");
	}

	/// Tests L4+ hijacking (lazy hex encoding).
	#[tokio::test]
	async fn test_l4p_hijacking() {
		let mut kv = KvStore::new();
		let mut payloads = HashMap::new();
		let data = vec![0xDE, 0xAD, 0xBE, 0xEF];
		payloads.insert("tls.clienthello".to_string(), Bytes::from(data));

		let mut context = SimpleContext {
			kv: &mut kv,
			payloads: Some(&payloads),
		};

		// 1. Resolve via hijack
		let result = context.get("tls.clienthello").await;
		assert_eq!(result, "deadbeef");

		// 2. Verify it was cached back to KV
		assert_eq!(kv.get("tls.clienthello"), Some(&"deadbeef".to_string()));
	}
}
