/* src/engine/key_scoping.rs */

/// Internal helper to push a sanitized plugin name (dots replaced with underscores)
/// directly into a buffer, avoiding intermediate String allocations.
fn push_sanitized(target: &mut String, name: &str) {
	for c in name.chars() {
		if c == '.' {
			target.push('_');
		} else {
			target.push(c);
		}
	}
}

/// Generates a namespaced key for plugin outputs based on the current execution flow path.
///
/// # Format
/// `plugin.{flow_path}.{sanitized_plugin_name}.{key}`
#[must_use] 
pub fn format_scoped_key(flow_path: &str, plugin_name: &str, key: &str) -> String {
	// Pre-calculate capacity to perform exactly one heap allocation.
	// "plugin." (7) + flow_path + plugin_name + key + possible dots
	let capacity = 7 + flow_path.len() + plugin_name.len() + key.len() + 3;
	let mut s = String::with_capacity(capacity);

	s.push_str("plugin.");
	if !flow_path.is_empty() {
		s.push_str(flow_path);
		s.push('.');
	}
	push_sanitized(&mut s, plugin_name);
	s.push('.');
	s.push_str(key);
	s
}

/// Helper to generate the next path segment for recursion.
/// Format: {current_path}.{sanitized_plugin_name}.{branch}
#[must_use] 
pub fn next_path(current_path: &str, plugin_name: &str, branch: &str) -> String {
	// Pre-calculate capacity to perform exactly one heap allocation.
	let capacity = current_path.len() + plugin_name.len() + branch.len() + 2;
	let mut s = String::with_capacity(capacity);

	if !current_path.is_empty() {
		s.push_str(current_path);
		s.push('.');
	}
	push_sanitized(&mut s, plugin_name);
	s.push('.');
	s.push_str(branch);
	s
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Tests name sanitization.
	#[test]
	fn test_sanitize() {
		let mut s = String::new();
		push_sanitized(&mut s, "internal.protocol.detect");
		assert_eq!(s, "internal_protocol_detect");
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
