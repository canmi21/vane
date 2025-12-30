/* src/modules/plugins/common/matcher.rs */

use crate::modules::plugins::model::{
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
			&format!(
				"⚙ Match Plugin: Comparing Left='{}' with Right='{}' (Op: '{}')",
				left, right, operator
			),
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
			// Future expansion (Regex, Numeric comparisons, etc.)
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
