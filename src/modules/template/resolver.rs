/* src/modules/template/resolver.rs */

use super::context::TemplateContext;
use super::parser::TemplateNode;

/// Resolve AST to final string
/// Never fails - returns original template string if key not found
pub async fn resolve_ast(
	nodes: &[TemplateNode],
	context: &mut dyn TemplateContext,
	depth: usize,
	max_depth: usize,
	max_size: usize,
) -> String {
	resolve_ast_with_depth(nodes, context, depth, max_depth, max_size).await
}

/// Internal resolver with depth tracking
fn resolve_ast_with_depth<'a>(
	nodes: &'a [TemplateNode],
	context: &'a mut dyn TemplateContext,
	depth: usize,
	max_depth: usize,
	max_size: usize,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send + 'a>> {
	Box::pin(async move {
		// Prevent infinite recursion
		if depth > max_depth {
			fancy_log::log(
				fancy_log::LogLevel::Error,
				&format!("✗ Template recursion depth limit ({}) exceeded", max_depth),
			);
			return String::new();
		}

		let mut result = String::new();

		for node in nodes {
			match node {
				TemplateNode::Text(s) => {
					if result.len() + s.len() > max_size {
						fancy_log::log(
							fancy_log::LogLevel::Error,
							&format!("✗ Template result size limit ({}) exceeded", max_size),
						);
						return result;
					}
					result.push_str(s);
				}
				TemplateNode::Variable { parts } => {
					// Recursively resolve nested parts to get the key name
					let key = resolve_ast_with_depth(parts, context, depth + 1, max_depth, max_size).await;

					// Template Injection Protection:
					// Key names must not contain '{' or '}'. If they do, it means dynamic data
					// contains template syntax. We refuse to resolve such keys to prevent
					// unauthorized access to other KV variables.
					if key.contains('{') || key.contains('}') {
						fancy_log::log(
							fancy_log::LogLevel::Error,
							&format!(
								"✗ Security: Template injection attempt detected in key name: '{}'. Refusing lookup.",
								key
							),
						);
						// Return the original template format to signify it wasn't resolved
						result.push_str("{{");
						result.push_str(&key);
						result.push_str("}}");
						continue;
					}

					// Lookup in context (never fails, returns original on error)
					let value = context.get(&key).await;

					if result.len() + value.len() > max_size {
						fancy_log::log(
							fancy_log::LogLevel::Error,
							&format!("✗ Template result size limit ({}) exceeded", max_size),
						);
						return result;
					}
					result.push_str(&value);
				}
			}
		}

		result
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::modules::kv::KvStore;
	use crate::modules::template::context::SimpleContext;
	use crate::modules::template::parser::parse_template;

	/// Tests resolving simple variable.
	#[tokio::test]
	async fn test_resolve_simple() {
		let mut kv = KvStore::new();
		kv.insert("key".to_string(), "value".to_string());

		let mut context = SimpleContext { kv: &kv };
		let ast = parse_template("{{key}}").unwrap();
		let result = resolve_ast(&ast, &mut context, 0, 5, 65536).await;

		assert_eq!(result, "value");
	}

	/// Tests resolving concatenated variables.
	#[tokio::test]
	async fn test_resolve_concatenation() {
		let mut kv = KvStore::new();
		kv.insert("conn.ip".to_string(), "1.2.3.4".to_string());
		kv.insert("conn.port".to_string(), "8080".to_string());

		let mut context = SimpleContext { kv: &kv };
		let ast = parse_template("{{conn.ip}}:{{conn.port}}").unwrap();
		let result = resolve_ast(&ast, &mut context, 0, 5, 65536).await;

		assert_eq!(result, "1.2.3.4:8080");
	}

	/// Tests resolving nested variables.
	#[tokio::test]
	async fn test_resolve_nested() {
		let mut kv = KvStore::new();
		kv.insert("conn.protocol".to_string(), "http".to_string());
		kv.insert("kv.http_backend".to_string(), "backend-01".to_string());

		let mut context = SimpleContext { kv: &kv };
		let ast = parse_template("{{kv.{{conn.protocol}}_backend}}").unwrap();
		let result = resolve_ast(&ast, &mut context, 0, 5, 65536).await;

		assert_eq!(result, "backend-01");
	}

	/// Tests resolving complex nested template.
	#[tokio::test]
	async fn test_resolve_complex() {
		let mut kv = KvStore::new();
		kv.insert("geo.country".to_string(), "US".to_string());
		kv.insert("kv.US_domain".to_string(), "api.example.com".to_string());

		let mut context = SimpleContext { kv: &kv };
		let ast = parse_template("https://{{kv.{{geo.country}}_domain}}/api").unwrap();
		let result = resolve_ast(&ast, &mut context, 0, 5, 65536).await;

		assert_eq!(result, "https://api.example.com/api");
	}

	/// Tests that missing keys return original template.
	#[tokio::test]
	async fn test_resolve_missing_key() {
		let kv = KvStore::new();
		let mut context = SimpleContext { kv: &kv };
		let ast = parse_template("{{missing}}").unwrap();
		let result = resolve_ast(&ast, &mut context, 0, 5, 65536).await;

		assert_eq!(result, "{{missing}}");
	}

	/// Tests empty AST.
	#[tokio::test]
	async fn test_resolve_empty() {
		let kv = KvStore::new();
		let mut context = SimpleContext { kv: &kv };
		let result = resolve_ast(&[], &mut context, 0, 5, 65536).await;

		assert_eq!(result, "");
	}

	/// Tests plain text without variables.
	#[tokio::test]
	async fn test_resolve_plain_text() {
		let kv = KvStore::new();
		let mut context = SimpleContext { kv: &kv };
		let ast = parse_template("plain text").unwrap();
		let result = resolve_ast(&ast, &mut context, 0, 5, 65536).await;

		assert_eq!(result, "plain text");
	}

	/// Tests protection against template injection in key names.
	#[tokio::test]
	async fn test_resolve_injection_attempt() {
		let mut kv = KvStore::new();
		// Injected value that looks like a template
		kv.insert("user_input".to_string(), "{{system.token}}".to_string());
		// A safe token we don't want to leak
		kv.insert("system.token".to_string(), "SECRET".to_string());

		let mut context = SimpleContext { kv: &kv };

		// Template tries to use user_input as part of a key
		// AST for: {{prefix.{{user_input}}}}
		let ast = parse_template("{{prefix.{{user_input}}}}").unwrap();
		let result = resolve_ast(&ast, &mut context, 0, 5, 65536).await;

		// The resolved key name would be "prefix.{{system.token}}"
		// Because it contains '{', the resolver should refuse to lookup and return it as text.
		assert_eq!(result, "{{prefix.{{system.token}}}}");
	}
}
