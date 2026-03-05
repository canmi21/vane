pub mod source;

use ahash::AHashMap;
use bytes::Bytes;
use nvr::{Config as NvrConfig, NotFound, resolve as nvr_resolve};
use serde_resolve::{
	Config as SerdeConfig, Resolved as SerdeResolved, json::resolve as serde_json_resolve,
};
use std::sync::Arc;
use tokio::sync::RwLock;
use varchain::{Scope, Source, SourceFuture};

pub use source::L4PlusSource;
use vane_primitives::kv::KvStore;

/// Returns the maximum allowed recursion depth for template and JSON resolution.
/// Configurable via `MAX_TEMPLATE_DEPTH` environment variable.
fn get_max_depth() -> usize {
	envflag::get::<usize>("MAX_TEMPLATE_DEPTH", 5)
}

/// Returns the maximum allowed size (in bytes) for a resolved template string.
/// Configurable via `MAX_TEMPLATE_RESULT_SIZE` environment variable.
fn get_max_size() -> usize {
	envflag::get::<usize>("MAX_TEMPLATE_RESULT_SIZE", 65536)
}

/// Helper struct for Kv source with Arc
struct AsyncKvSource {
	kv: Arc<RwLock<KvStore>>,
}

impl Source for AsyncKvSource {
	fn get(&self, key: &str) -> SourceFuture<'_, String> {
		let key = key.to_owned();
		let kv = self.kv.clone();
		Box::pin(async move { kv.read().await.get(&key).cloned().into() })
	}
}

/// Build scope for L4/L4+ (with optional payloads for hijacking)
pub fn build_l4_scope(
	kv: Arc<RwLock<KvStore>>,
	payloads: Option<Arc<AHashMap<String, Bytes>>>,
) -> Scope {
	let mut scope = Scope::new();

	// 1. L4+ hijacking source (highest priority)
	if let Some(payloads) = payloads {
		scope = scope.push(L4PlusSource {
			kv: kv.clone(),
			payloads,
		});
	}

	// 2. Standard KV source
	scope.push(AsyncKvSource { kv })
}

/// Resolve single template string
pub async fn resolve_template(template: &str, scope: &Scope) -> String {
	let config = NvrConfig {
		parse: Default::default(),
		max_resolve_depth: get_max_depth(),
		max_result_size: get_max_size(),
		not_found: NotFound::ReturnOriginal,
	};

	match nvr_resolve(template, scope, &config).await {
		Ok(resolved) => resolved,
		Err(e) => {
			fancy_log::log(
				fancy_log::LogLevel::Warn,
				&format!("⚠ Template resolve error: {e}, returning original string"),
			);
			template.to_owned()
		}
	}
}

/// Resolve all strings in JSON Value map
pub async fn resolve_inputs(
	inputs: &std::collections::HashMap<String, serde_json::Value>,
	scope: &Scope,
) -> std::collections::HashMap<String, serde_json::Value> {
	let nvr_config = NvrConfig {
		parse: Default::default(),
		max_resolve_depth: get_max_depth(),
		max_result_size: get_max_size(),
		not_found: NotFound::ReturnOriginal,
	};

	let serde_config = SerdeConfig {
		max_depth: 32,
		resolve_keys: false,
	};

	let mut resolved = std::collections::HashMap::with_capacity(inputs.len());

	for (key, value) in inputs {
		// Use closure as Resolver for serde_resolve
		let resolver = |s: &str| {
			let s_owned = s.to_owned();
			let scope = scope.clone();
			let nvr_config = nvr_config.clone();
			async move {
				match nvr_resolve(&s_owned, &scope, &nvr_config).await {
					Ok(res) => {
						if res == s_owned {
							Ok::<SerdeResolved, std::convert::Infallible>(SerdeResolved::Unchanged)
						} else {
							Ok(SerdeResolved::Changed(res))
						}
					}
					Err(_) => Ok(SerdeResolved::Unchanged),
				}
			}
		};

		match serde_json_resolve(value.clone(), &resolver, &serde_config).await {
			Ok(res) => {
				resolved.insert(key.clone(), res);
			}
			Err(e) => {
				fancy_log::log(
					fancy_log::LogLevel::Error,
					&format!("✗ JSON resolve error for key {key}: {e}"),
				);
				resolved.insert(key.clone(), value.clone());
			}
		}
	}

	resolved
}

#[cfg(test)]
mod tests {
	use super::*;
	use vane_primitives::kv::KvStore;

	fn init() {
		envflag::init().ok();
	}

	#[tokio::test]
	async fn test_resolve_template_simple() {
		init();
		let mut kv = KvStore::new();
		kv.insert("key".to_owned(), "value".to_owned());
		let kv = Arc::new(RwLock::new(kv));

		let scope = build_l4_scope(kv, None);
		let result = resolve_template("{{key}}", &scope).await;

		assert_eq!(result, "value");
	}

	#[tokio::test]
	async fn test_resolve_template_concatenation() {
		init();
		let mut kv = KvStore::new();
		kv.insert("conn.ip".to_owned(), "1.2.3.4".to_owned());
		kv.insert("conn.port".to_owned(), "8080".to_owned());
		let kv = Arc::new(RwLock::new(kv));

		let scope = build_l4_scope(kv, None);
		let result = resolve_template("{{conn.ip}}:{{conn.port}}", &scope).await;

		assert_eq!(result, "1.2.3.4:8080");
	}

	#[tokio::test]
	async fn test_resolve_template_nested() {
		init();
		let mut kv = KvStore::new();
		kv.insert("conn.protocol".to_owned(), "http".to_owned());
		kv.insert("kv.http_backend".to_owned(), "backend-01".to_owned());
		let kv = Arc::new(RwLock::new(kv));

		let scope = build_l4_scope(kv, None);
		let result = resolve_template("{{kv.{{conn.protocol}}_backend}}", &scope).await;

		assert_eq!(result, "backend-01");
	}

	#[tokio::test]
	async fn test_resolve_inputs() {
		init();
		let mut kv = KvStore::new();
		kv.insert("host".to_owned(), "example.com".to_owned());
		kv.insert("port".to_owned(), "443".to_owned());
		let kv = Arc::new(RwLock::new(kv));

		let inputs = std::collections::HashMap::from([(
			"url".to_owned(),
			serde_json::json!("https://{{host}}:{{port}}"),
		)]);

		let scope = build_l4_scope(kv, None);
		let resolved = resolve_inputs(&inputs, &scope).await;

		assert_eq!(
			resolved["url"],
			serde_json::json!("https://example.com:443")
		);
	}
}
