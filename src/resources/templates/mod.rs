pub mod source;

// L4 generic parts live in vane-engine
pub use source::{HttpSource, L4PlusSource};
pub use vane_engine::templates::{build_l4_scope, resolve_inputs, resolve_template};

// L7-specific parts now live in vane-app
pub use vane_app::templates::build_l7_scope;

#[cfg(test)]
mod tests {
	use super::*;
	use std::sync::Arc;
	use tokio::sync::RwLock;
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
