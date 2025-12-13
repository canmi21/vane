/* src/modules/plugins/protocol/tls/alpn.rs */

use super::clienthello;
use crate::modules::plugins::model::{
	Middleware, MiddlewareOutput, ParamDef, ParamType, Plugin, ResolvedInputs,
};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde_json::Value;
use std::{any::Any, borrow::Cow};

pub struct TlsAlpnPlugin;

impl Plugin for TlsAlpnPlugin {
	fn name(&self) -> &'static str {
		"internal.protocol.tls.alpn"
	}

	fn params(&self) -> Vec<ParamDef> {
		vec![
			ParamDef {
				name: "clienthello".into(),
				required: true,
				param_type: ParamType::String,
			},
			ParamDef {
				name: "match".into(),
				required: true,
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
}

#[async_trait]
impl Middleware for TlsAlpnPlugin {
	fn output(&self) -> Vec<Cow<'static, str>> {
		vec!["true".into(), "false".into()]
	}

	async fn execute(&self, inputs: ResolvedInputs) -> Result<MiddlewareOutput> {
		let hex_data = inputs
			.get("clienthello")
			.and_then(Value::as_str)
			.ok_or_else(|| anyhow!("Input 'clienthello' missing"))?;

		let target_alpn = inputs
			.get("match")
			.and_then(Value::as_str)
			.ok_or_else(|| anyhow!("Input 'match' missing"))?;

		let payload = hex::decode(hex_data).map_err(|e| anyhow!("Invalid clienthello hex: {}", e))?;

		let protocols = clienthello::extract_alpn(&payload)?;

		let branch = if protocols.iter().any(|p| p == target_alpn) {
			"true"
		} else {
			"false"
		};

		Ok(MiddlewareOutput {
			branch: branch.into(),
			store: None,
		})
	}
}
