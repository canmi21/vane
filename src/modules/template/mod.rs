/* src/modules/template/mod.rs */

pub mod context;
pub mod hijack;
pub mod parser;
pub mod resolver;

pub use context::TemplateContext;

use serde_json::{Map, Value};
use std::collections::HashMap;

/// High-level API: Parse and resolve template string
/// Returns original string on parse error (with log)
pub async fn resolve_template(template: &str, context: &mut dyn TemplateContext) -> String {
	match parser::parse_template(template) {
		Ok(ast) => resolver::resolve_ast(&ast, context).await,
		Err(e) => {
			fancy_log::log(
				fancy_log::LogLevel::Warn,
				&format!("⚠ Template parse error: {}, returning original string", e),
			);
			template.to_string()
		}
	}
}

/// Helper for resolving plugin inputs (HashMap<String, Value>)
/// Never fails - returns original values on error
pub async fn resolve_inputs(
	inputs: &HashMap<String, Value>,
	context: &mut dyn TemplateContext,
) -> HashMap<String, Value> {
	let mut resolved = HashMap::new();

	for (key, value) in inputs {
		let resolved_val = resolve_value_recursive(value, context).await;
		resolved.insert(key.clone(), resolved_val);
	}

	resolved
}

/// Recursive helper for JSON structures (Arrays, Objects)
fn resolve_value_recursive<'a>(
	value: &'a Value,
	context: &'a mut dyn TemplateContext,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Value> + Send + 'a>> {
	Box::pin(async move {
		match value {
			Value::String(s) => {
				let result = resolve_template(s, context).await;
				Value::String(result)
			}
			Value::Array(arr) => {
				let mut new_arr = Vec::with_capacity(arr.len());
				for item in arr {
					new_arr.push(resolve_value_recursive(item, context).await);
				}
				Value::Array(new_arr)
			}
			Value::Object(map) => {
				let mut new_map = Map::with_capacity(map.len());
				for (k, v) in map {
					new_map.insert(k.clone(), resolve_value_recursive(v, context).await);
				}
				Value::Object(new_map)
			}
			_ => value.clone(), // Numbers, Bools, Nulls are kept as-is
		}
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::modules::kv::KvStore;
	use context::SimpleContext;

	/// Tests resolve_template with simple variable.
	#[tokio::test]
	async fn test_resolve_template_simple() {
		let mut kv = KvStore::new();
		kv.insert("key".to_string(), "value".to_string());

		let mut context = SimpleContext { kv: &kv };
		let result = resolve_template("{{key}}", &mut context).await;

		assert_eq!(result, "value");
	}

	/// Tests resolve_template with concatenation.
	#[tokio::test]
	async fn test_resolve_template_concatenation() {
		let mut kv = KvStore::new();
		kv.insert("conn.ip".to_string(), "1.2.3.4".to_string());
		kv.insert("conn.port".to_string(), "8080".to_string());

		let mut context = SimpleContext { kv: &kv };
		let result = resolve_template("{{conn.ip}}:{{conn.port}}", &mut context).await;

		assert_eq!(result, "1.2.3.4:8080");
	}

	/// Tests resolve_template with nested template.
	#[tokio::test]
	async fn test_resolve_template_nested() {
		let mut kv = KvStore::new();
		kv.insert("conn.protocol".to_string(), "http".to_string());
		kv.insert("kv.http_backend".to_string(), "backend-01".to_string());

		let mut context = SimpleContext { kv: &kv };
		let result = resolve_template("{{kv.{{conn.protocol}}_backend}}", &mut context).await;

		assert_eq!(result, "backend-01");
	}

	/// Tests resolve_inputs with HashMap.
	#[tokio::test]
	async fn test_resolve_inputs() {
		let mut kv = KvStore::new();
		kv.insert("host".to_string(), "example.com".to_string());
		kv.insert("port".to_string(), "443".to_string());

		let mut inputs = HashMap::new();
		inputs.insert(
			"url".to_string(),
			Value::String("https://{{host}}:{{port}}".to_string()),
		);

		let mut context = SimpleContext { kv: &kv };
		let resolved = resolve_inputs(&inputs, &mut context).await;

		assert_eq!(
			resolved.get("url"),
			Some(&Value::String("https://example.com:443".to_string()))
		);
	}

	/// Tests resolve_inputs with nested JSON.
	#[tokio::test]
	async fn test_resolve_inputs_nested_json() {
		let mut kv = KvStore::new();
		kv.insert("name".to_string(), "test".to_string());

		let mut inputs = HashMap::new();
		inputs.insert(
			"config".to_string(),
			serde_json::json!({
					"title": "{{name}}",
					"nested": {
							"value": "{{name}}-value"
					},
					"array": ["{{name}}-1", "{{name}}-2"]
			}),
		);

		let mut context = SimpleContext { kv: &kv };
		let resolved = resolve_inputs(&inputs, &mut context).await;

		let config = resolved.get("config").unwrap();
		assert_eq!(config["title"], "test");
		assert_eq!(config["nested"]["value"], "test-value");
		assert_eq!(config["array"][0], "test-1");
		assert_eq!(config["array"][1], "test-2");
	}

	/// Tests that parse errors return original string.
	#[tokio::test]
	async fn test_resolve_template_parse_error() {
		let kv = KvStore::new();
		let mut context = SimpleContext { kv: &kv };

		// Unclosed variable
		let result = resolve_template("{{key", &mut context).await;
		assert_eq!(result, "{{key");
	}

	/// Tests plain text without variables.
	#[tokio::test]
	async fn test_resolve_template_plain_text() {
		let kv = KvStore::new();
		let mut context = SimpleContext { kv: &kv };
		let result = resolve_template("plain text", &mut context).await;

		assert_eq!(result, "plain text");
	}

	/// Tests empty template.
	#[tokio::test]
	async fn test_resolve_template_empty() {
		let kv = KvStore::new();
		let mut context = SimpleContext { kv: &kv };
		let result = resolve_template("", &mut context).await;

		assert_eq!(result, "");
	}
}
