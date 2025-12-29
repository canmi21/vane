/* src/modules/stack/protocol/application/template.rs */

use super::container::Container;
use anyhow::Result;
use fancy_log::{LogLevel, log};
use http::HeaderMap;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

/// Resolves input templates recursively.
/// Supports string replacement in Values, Arrays, and Objects.
pub async fn resolve_inputs(
	inputs: &HashMap<String, Value>,
	container: &mut Container,
) -> Result<HashMap<String, Value>> {
	let mut resolved = HashMap::new();
	for (k, v) in inputs {
		let resolved_val = resolve_recursive(v, container).await?;
		resolved.insert(k.clone(), resolved_val);
	}
	Ok(resolved)
}

/// Recursive helper to traverse JSON structures.
/// Manually boxed to handle async recursion without external crates.
fn resolve_recursive<'a>(
	value: &'a Value,
	container: &'a mut Container,
) -> Pin<Box<dyn Future<Output = Result<Value>> + Send + 'a>> {
	Box::pin(async move {
		match value {
			Value::String(s) => {
				// Check for template syntax "{{key}}"
				if s.starts_with("{{") && s.ends_with("}}") {
					let lookup_key = &s[2..s.len() - 2];
					let resolved_str = resolve_key(lookup_key, container).await?;
					Ok(Value::String(resolved_str))
				} else {
					Ok(Value::String(s.clone()))
				}
			}
			Value::Array(arr) => {
				let mut new_arr = Vec::with_capacity(arr.len());
				for item in arr {
					new_arr.push(resolve_recursive(item, container).await?);
				}
				Ok(Value::Array(new_arr))
			}
			Value::Object(map) => {
				let mut new_map = Map::with_capacity(map.len());
				for (k, v) in map {
					new_map.insert(k.clone(), resolve_recursive(v, container).await?);
				}
				Ok(Value::Object(new_map))
			}
			_ => Ok(value.clone()), // Numbers, Bools, Nulls are kept as-is
		}
	})
}

/// Core lookup logic for a specific template key.
async fn resolve_key(key: &str, container: &mut Container) -> Result<String> {
	// Lazy Buffering
	if matches!(
		key,
		"req.body" | "req.body_hex" | "res.body" | "res.body_hex"
	) {
		let bytes = if key.starts_with("req.") {
			container.force_buffer_request().await?
		} else {
			container.force_buffer_response().await?
		};

		let is_hex = key.ends_with("_hex");
		if is_hex {
			return Ok(hex::encode(bytes));
		} else {
			return Ok(String::from_utf8_lossy(bytes).to_string());
		}
	}

	// On-Demand Header Extraction
	if let Some(header_name) = key.strip_prefix("req.header.") {
		return Ok(get_header_value(&container.request_headers, header_name));
	}
	if let Some(header_name) = key.strip_prefix("res.header.") {
		return Ok(get_header_value(&container.response_headers, header_name));
	}

	if key == "req.headers" {
		return Ok(format!("{:?}", container.request_headers));
	}
	if key == "res.headers" {
		return Ok(format!("{:?}", container.response_headers));
	}

	// KV Store Fallback
	if let Some(kv_val) = container.kv.get(key) {
		Ok(kv_val.clone())
	} else {
		log(
			LogLevel::Warn,
			&format!(
				"⚠ Template '{}' not found in Context KV or Container Headers.",
				key
			),
		);
		Ok(String::new())
	}
}

fn get_header_value(map: &HeaderMap, key_name: &str) -> String {
	match map.get(key_name) {
		Some(val) => val.to_str().unwrap_or("").to_string(),
		None => "".to_string(),
	}
}
