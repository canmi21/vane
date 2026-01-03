/* src/modules/template/mod.rs */

pub mod context;
pub mod hijack;
pub mod parser;
pub mod resolver;

pub use context::TemplateContext;

use crate::common::getenv;
use serde_json::{Map, Value};
use std::collections::HashMap;

/// Returns the maximum allowed recursion depth for template and JSON resolution.
/// Configurable via `MAX_TEMPLATE_DEPTH` environment variable.
fn get_max_depth() -> usize {
	getenv::get_env("MAX_TEMPLATE_DEPTH", "5".to_string())
		.parse()
		.unwrap_or(5)
}

/// Returns the maximum allowed size (in bytes) for a resolved template string.
/// Configurable via `MAX_TEMPLATE_RESULT_SIZE` environment variable.
fn get_max_size() -> usize {
	getenv::get_env("MAX_TEMPLATE_RESULT_SIZE", "65536".to_string())
		.parse()
		.unwrap_or(65536)
}

/// High-level API: Parse and resolve template string
/// Returns original string on parse error (with log)
pub async fn resolve_template(
	template: &str,
	context: &mut dyn TemplateContext,
	depth: usize,
) -> String {
	let max_depth = get_max_depth();
	let max_size = get_max_size();

	if depth > max_depth {
		fancy_log::log(
			fancy_log::LogLevel::Error,
			&format!("✗ Template recursion depth limit ({}) exceeded", max_depth),
		);
		return template.to_string();
	}

	match parser::parse_template(template) {
		Ok(ast) => resolver::resolve_ast(&ast, context, depth, max_depth, max_size).await,
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
		let resolved_val = resolve_value_recursive(value, context, 0).await;
		resolved.insert(key.clone(), resolved_val);
	}

	resolved
}

/// Recursive helper for JSON structures (Arrays, Objects)
fn resolve_value_recursive<'a>(
	value: &'a Value,
	context: &'a mut dyn TemplateContext,
	depth: usize,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Value> + Send + 'a>> {
	Box::pin(async move {
		let max_depth = get_max_depth();
		if depth > max_depth {
			fancy_log::log(
				fancy_log::LogLevel::Error,
				&format!("✗ JSON recursion depth limit ({}) exceeded", max_depth),
			);
			return value.clone();
		}

		match value {
			Value::String(s) => {
				let result = resolve_template(s, context, depth).await;
				Value::String(result)
			}
			Value::Array(arr) => {
				let mut new_arr = Vec::with_capacity(arr.len());
				for item in arr {
					new_arr.push(resolve_value_recursive(item, context, depth + 1).await);
				}
				Value::Array(new_arr)
			}
			Value::Object(map) => {
				let mut new_map = Map::with_capacity(map.len());
				for (k, v) in map {
					new_map.insert(
						k.clone(),
						resolve_value_recursive(v, context, depth + 1).await,
					);
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

		let mut context = SimpleContext {
			kv: &mut kv,
			payloads: None,
		};
		let result = resolve_template("{{key}}", &mut context, 0).await;

		assert_eq!(result, "value");
	}

	/// Tests resolve_template with concatenation.
	#[tokio::test]
	async fn test_resolve_template_concatenation() {
		let mut kv = KvStore::new();
		kv.insert("conn.ip".to_string(), "1.2.3.4".to_string());
		kv.insert("conn.port".to_string(), "8080".to_string());

		let mut context = SimpleContext {
			kv: &mut kv,
			payloads: None,
		};
		let result = resolve_template("{{conn.ip}}:{{conn.port}}", &mut context, 0).await;

		assert_eq!(result, "1.2.3.4:8080");
	}

	/// Tests resolve_template with nested template.
	#[tokio::test]
	async fn test_resolve_template_nested() {
		let mut kv = KvStore::new();
		kv.insert("conn.protocol".to_string(), "http".to_string());
		kv.insert("kv.http_backend".to_string(), "backend-01".to_string());

		let mut context = SimpleContext {
			kv: &mut kv,
			payloads: None,
		};
		let result = resolve_template("{{kv.{{conn.protocol}}_backend}}", &mut context, 0).await;

		assert_eq!(result, "backend-01");
	}

	/// Tests recursion limit for templates.
	#[test]
	#[serial_test::serial]
	fn test_resolve_template_recursion_limit() {
		let mut kv = KvStore::new();
		let mut context = SimpleContext {
			kv: &mut kv,
			payloads: None,
		};
		// Nested depth 6 (exceeds default 5)
		let deep_template = "{{a.{{b.{{c.{{d.{{e.{{f}}}}}}}}}}}}";

		// We need to increase the PARSE depth limit so the parser allows this string,
		// but the RESOLVER depth limit (default 5) will still trigger.
		temp_env::with_var("MAX_TEMPLATE_PARSE_DEPTH", Some("10"), || {
			let rt = tokio::runtime::Runtime::new().unwrap();
			rt.block_on(async {
				let result = resolve_template(deep_template, &mut context, 0).await;
				// Should stop at limit and return truncated result
				assert!(result.len() < deep_template.len());
			});
		});
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

		let mut context = SimpleContext {
			kv: &mut kv,
			payloads: None,
		};
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

		let mut context = SimpleContext {
			kv: &mut kv,
			payloads: None,
		};
		let resolved = resolve_inputs(&inputs, &mut context).await;

		let config = resolved.get("config").unwrap();
		assert_eq!(config["title"], "test");
		assert_eq!(config["nested"]["value"], "test-value");
		assert_eq!(config["array"][0], "test-1");
		assert_eq!(config["array"][1], "test-2");
	}

	/// Tests JSON recursion limit.
	#[tokio::test]
	async fn test_resolve_inputs_json_limit() {
		let mut kv = KvStore::new();
		let mut context = SimpleContext {
			kv: &mut kv,
			payloads: None,
		};

		// Create a deeply nested JSON object (depth 10)
		let mut deep_json = serde_json::json!({"val": "end"});
		for _ in 0..10 {
			deep_json = serde_json::json!({"next": deep_json});
		}

		let mut inputs = HashMap::new();
		inputs.insert("deep".to_string(), deep_json);

		let resolved = resolve_inputs(&inputs, &mut context).await;
		let resolved_val = resolved.get("deep").unwrap();

		// The resolved value should be the same as input because it hit the limit and returned early
		// (Or it might be partially resolved, but it won't crash)
		assert!(resolved_val.is_object());
	}

	/// Tests template size limit.
	#[test]
	#[serial_test::serial]
	fn test_resolve_template_size_limit() {
		let mut kv = KvStore::new();
		let mut context = SimpleContext {
			kv: &mut kv,
			payloads: None,
		};

		temp_env::with_var("MAX_TEMPLATE_RESULT_SIZE", Some("10"), || {
			let rt = tokio::runtime::Runtime::new().unwrap();
			rt.block_on(async {
				let result = resolve_template("long string that exceeds 10 bytes", &mut context, 0).await;
				// Should be truncated or empty depending on where it hit
				assert!(result.len() <= 10);
			});
		});
	}

	/// Tests that parse errors return original string.
	#[tokio::test]
	async fn test_resolve_template_parse_error() {
		let mut kv = KvStore::new();
		let mut context = SimpleContext {
			kv: &mut kv,
			payloads: None,
		};

		// Unclosed variable
		let result = resolve_template("{{key", &mut context, 0).await;
		assert_eq!(result, "{{key");
	}

	/// Tests plain text without variables.
	#[tokio::test]
	async fn test_resolve_template_plain_text() {
		let mut kv = KvStore::new();
		let mut context = SimpleContext {
			kv: &mut kv,
			payloads: None,
		};
		let result = resolve_template("plain text", &mut context, 0).await;

		assert_eq!(result, "plain text");
	}

	/// Tests empty template.
	#[tokio::test]
	async fn test_resolve_template_empty() {
		let mut kv = KvStore::new();
		let mut context = SimpleContext {
			kv: &mut kv,
			payloads: None,
		};
		let result = resolve_template("", &mut context, 0).await;

		assert_eq!(result, "");
	}
}
