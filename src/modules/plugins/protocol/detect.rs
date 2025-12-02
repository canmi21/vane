/* src/modules/plugins/protocol/detect.rs */

use crate::modules::plugins::model::{
	Middleware, MiddlewareOutput, ParamDef, ParamType, Plugin, ResolvedInputs,
};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde_json::Value;
use std::any::Any;

/// Core detection logic. This is a pure, stateless function.
fn detect(payload: &[u8], method: &str) -> bool {
	if payload.is_empty() {
		return false;
	}
	match method {
		"http" => {
			payload.starts_with(b"GET ")
				|| payload.starts_with(b"POST ")
				|| payload.starts_with(b"PUT ")
				|| payload.starts_with(b"DELETE ")
				|| payload.starts_with(b"HEAD ")
				|| payload.starts_with(b"OPTIONS ")
				|| payload.starts_with(b"PATCH ")
		}
		"tls" => payload.starts_with(&[0x16, 0x03]) && payload.len() > 3,
		_ => false,
	}
}

/// A built-in Middleware plugin for basic L4 protocol detection.
pub struct ProtocolDetectPlugin;

impl Plugin for ProtocolDetectPlugin {
	fn name(&self) -> &'static str {
		"internal.protocol.detect"
	}

	fn params(&self) -> Vec<ParamDef> {
		vec![
			ParamDef {
				name: "method",
				required: true,
				param_type: ParamType::String,
			},
			ParamDef {
				name: "payload",
				required: true,
				param_type: ParamType::Bytes,
			},
		]
	}

	fn as_any(&self) -> &dyn Any {
		self
	}

	fn as_middleware(&self) -> Option<&dyn Middleware> {
		Some(self)
	}
}

#[async_trait]
impl Middleware for ProtocolDetectPlugin {
	fn output(&self) -> Vec<&'static str> {
		vec!["true", "false"]
	}

	async fn execute(&self, inputs: ResolvedInputs) -> Result<MiddlewareOutput> {
		let method = inputs
			.get("method")
			.and_then(Value::as_str)
			.ok_or_else(|| anyhow!("Resolved input 'method' is missing or not a string"))?;

		let payload_hex = inputs
			.get("payload")
			.and_then(Value::as_str)
			.ok_or_else(|| anyhow!("Resolved input 'payload' is missing or not a string"))?;
		let payload = hex::decode(payload_hex)?;

		let result = detect(&payload, method);
		let branch = if result { "true" } else { "false" };

		Ok(MiddlewareOutput {
			branch,
			write_to_kv: None,
		})
	}
}
