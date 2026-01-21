/* src/plugins/middleware/matcher.rs */

use crate::engine::interfaces::{
	GenericMiddleware, Middleware, MiddlewareOutput, ParamDef, ParamType, Plugin, ResolvedInputs,
};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use fancy_log::{LogLevel, log};
use serde_json::Value;
use std::{any::Any, borrow::Cow};

/// A universal matching middleware.
/// Compares two values and branches based on the boolean result.
///
/// # Logic
/// If `left` [operator] `right` is true, returns branch "true".
/// Otherwise, returns branch "false".
pub struct CommonMatchPlugin;

impl Plugin for CommonMatchPlugin {
	fn name(&self) -> &'static str {
		"internal.common.match"
	}

	fn params(&self) -> Vec<ParamDef> {
		vec![
			ParamDef {
				name: "left".into(),
				required: true,
				param_type: ParamType::String,
			},
			ParamDef {
				name: "right".into(),
				required: true,
				param_type: ParamType::String,
			},
			ParamDef {
				name: "operator".into(),
				required: false,
				param_type: ParamType::String,
			},
		]
	}

	fn as_any(&self) -> &dyn Any {
		self
	}

	fn as_middleware(&self) -> Option<&dyn Middleware> {
		Some(self)
	}

	fn as_generic_middleware(&self) -> Option<&dyn GenericMiddleware> {
		Some(self)
	}
}

#[async_trait]
impl GenericMiddleware for CommonMatchPlugin {
	fn output(&self) -> Vec<Cow<'static, str>> {
		vec!["true".into(), "false".into()]
	}

	async fn execute(&self, inputs: ResolvedInputs) -> Result<MiddlewareOutput> {
		let left = inputs
			.get("left")
			.and_then(Value::as_str)
			.ok_or_else(|| anyhow!("Input 'left' missing or not a string"))?;

		let right = inputs
			.get("right")
			.and_then(Value::as_str)
			.ok_or_else(|| anyhow!("Input 'right' missing or not a string"))?;

		// Normalize operator to lowercase for robust matching
		let operator = inputs
			.get("operator")
			.and_then(Value::as_str)
			.unwrap_or("==")
			.to_lowercase();

		log(
			LogLevel::Debug,
			&format!("⚙ Match Plugin: Comparing Left='{left}' with Right='{right}' (Op: '{operator}')"),
		);

		let result = match operator.as_str() {
			// Equality
			"==" | "eq" | "equal" | "equals" => left == right,
			// Inequality
			"!=" | "ne" | "notequal" | "not_equal" => left != right,
			// String Operations
			"contains" | "contain" => left.contains(right),
			"startswith" | "starts_with" => left.starts_with(right),
			"endswith" | "ends_with" => left.ends_with(right),
			// Regex Matching
			"regex" | "re" | "match" => match regex::Regex::new(right) {
				Ok(re) => re.is_match(left),
				Err(e) => {
					log(
						LogLevel::Error,
						&format!("✗ Invalid regex pattern '{right}': {e}"),
					);
					false
				}
			},
			_ => false,
		};

		Ok(MiddlewareOutput {
			branch: if result {
				"true".into()
			} else {
				"false".into()
			},
			store: None,
		})
	}
}

#[async_trait]
impl Middleware for CommonMatchPlugin {
	fn output(&self) -> Vec<Cow<'static, str>> {
		// Delegate to Generic implementation
		<Self as GenericMiddleware>::output(self)
	}

	async fn execute(&self, inputs: ResolvedInputs) -> Result<MiddlewareOutput> {
		// Delegate to Generic implementation
		<Self as GenericMiddleware>::execute(self, inputs).await
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::collections::HashMap;

	/// Tests that the matcher correctly handles various string operations.
	#[tokio::test]
	async fn test_string_matching_operators() {
		let plugin = CommonMatchPlugin;

		let cases = vec![
			// Equality
			("hello", "hello", "eq", "true"),
			("hello", "world", "eq", "false"),
			("hello", "HELLO", "eq", "false"),
			// Inequality
			("hello", "world", "ne", "true"),
			("hello", "hello", "ne", "false"),
			// Contains
			("hello world", "world", "contains", "true"),
			("hello world", "rust", "contains", "false"),
			// StartsWith
			("https://vane.com", "https://", "startswith", "true"),
			("https://vane.com", "http://", "startswith", "false"),
			("https://vane.com", "vane", "startswith", "false"),
			// EndsWith
			("image.png", ".png", "endswith", "true"),
			("image.png", ".jpg", "endswith", "false"),
			// Regex
			("vane-123", r"^vane-\d+$", "regex", "true"),
			("vane-abc", r"^vane-\d+$", "regex", "false"),
			("test", "[invalid", "regex", "false"), // Invalid regex should fail safely
		];

		for (left, right, op, expected) in cases {
			let mut inputs = HashMap::new();
			inputs.insert("left".to_string(), Value::String(left.to_string()));
			inputs.insert("right".to_string(), Value::String(right.to_string()));
			inputs.insert("operator".to_string(), Value::String(op.to_string()));

			let out = GenericMiddleware::execute(&plugin, inputs).await.unwrap();
			assert_eq!(
				out.branch, expected,
				"Failed match: '{}' {} '{}' should be {}",
				left, op, right, expected
			);
		}
	}

	/// Tests default operator (equality).
	#[tokio::test]
	async fn test_default_operator() {
		let plugin = CommonMatchPlugin;
		let mut inputs = HashMap::new();
		inputs.insert("left".to_string(), Value::String("a".to_string()));
		inputs.insert("right".to_string(), Value::String("a".to_string()));
		// No operator provided

		let out = GenericMiddleware::execute(&plugin, inputs).await.unwrap();
		assert_eq!(out.branch, "true");
	}
}
