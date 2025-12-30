/* src/modules/flow/key_scoping.rs */

/// Sanitizes the plugin name by replacing dots with underscores.
/// This prevents ambiguity in the dot-separated KV path.
/// Example: "internal.protocol.detect" -> "internal_protocol_detect"
fn sanitize_name(name: &str) -> String {
	name.replace('.', "_")
}

/// Generates a namespaced key for plugin outputs based on the current execution flow path.
///
/// # Format
/// `plugin.{flow_path}.{sanitized_plugin_name}.{key}`
///
/// # Example
/// Path: "internal_protocol_detect.true"
/// Current Plugin: "internal.common.ratelimit.sec"
/// Key: "limit"
/// Result: "plugin.internal_protocol_detect.true.internal_common_ratelimit_sec.limit"
pub fn format_scoped_key(flow_path: &str, plugin_name: &str, key: &str) -> String {
	let safe_plugin = sanitize_name(plugin_name);
	if flow_path.is_empty() {
		format!("plugin.{}.{}", safe_plugin, key)
	} else {
		format!("plugin.{}.{}.{}", flow_path, safe_plugin, key)
	}
}

/// Helper to generate the next path segment for recursion.
/// Format: {current_path}.{sanitized_plugin_name}.{branch}
pub fn next_path(current_path: &str, plugin_name: &str, branch: &str) -> String {
	let safe_plugin = sanitize_name(plugin_name);
	if current_path.is_empty() {
		format!("{}.{}", safe_plugin, branch)
	} else {
		format!("{}.{}.{}", current_path, safe_plugin, branch)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Tests name sanitization.
	#[test]
	fn test_sanitize() {
		assert_eq!(
			sanitize_name("internal.protocol.detect"),
			"internal_protocol_detect"
		);
	}

	/// Tests root level key generation.
	#[test]
	fn test_root_level_key() {
		let key = format_scoped_key("", "internal.protocol.detect", "method");
		assert_eq!(key, "plugin.internal_protocol_detect.method");
	}

	/// Tests nested level key generation.
	#[test]
	fn test_nested_level_key() {
		// Simulate: Detect -> True -> RateLimit
		let path = next_path("", "internal.protocol.detect", "true");
		assert_eq!(path, "internal_protocol_detect.true");

		let key = format_scoped_key(&path, "internal.common.ratelimit.sec", "count");
		assert_eq!(
			key,
			"plugin.internal_protocol_detect.true.internal_common_ratelimit_sec.count"
		);
	}

	/// Tests deeply nested path generation.
	#[test]
	fn test_deeply_nested_path() {
		// Detect -> True -> Auth -> Success
		let p1 = next_path("", "internal.detect", "true");
		let p2 = next_path(&p1, "external.auth", "success");

		assert_eq!(p2, "internal_detect.true.external_auth.success");
	}
}
