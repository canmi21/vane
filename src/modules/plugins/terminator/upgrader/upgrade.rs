/* src/modules/plugins/terminator/upgrader/upgrade.rs */

use crate::modules::{
	kv::KvStore,
	plugins::model::{
		ConnectionObject, Layer, ParamDef, ParamType, Plugin, ResolvedInputs, Terminator,
		TerminatorResult,
	},
};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use fancy_log::{LogLevel, log};
use serde_json::Value;
use std::any::Any;

pub struct UpgradePlugin;

impl Plugin for UpgradePlugin {
	fn name(&self) -> &'static str {
		"internal.transport.upgrade"
	}

	fn params(&self) -> Vec<ParamDef> {
		vec![ParamDef {
			name: "protocol".into(),
			required: true,
			param_type: ParamType::String,
		}]
	}

	fn as_any(&self) -> &dyn Any {
		self
	}

	fn as_terminator(&self) -> Option<&dyn Terminator> {
		Some(self)
	}
}

#[async_trait]
impl Terminator for UpgradePlugin {
	fn supported_layers(&self) -> Vec<Layer> {
		vec![Layer::L4, Layer::L4Plus]
	}

	async fn execute(
		&self,
		inputs: ResolvedInputs,
		_kv: &KvStore,
		conn: ConnectionObject,
	) -> Result<TerminatorResult> {
		let protocol = inputs
			.get("protocol")
			.and_then(Value::as_str)
			.ok_or_else(|| anyhow!("Resolved input 'protocol' is missing or not a string"))?;

		log(
			LogLevel::Debug,
			&format!("➜ Signal upgrade to protocol: {}", protocol),
		);

		Ok(TerminatorResult::Upgrade {
			protocol: protocol.to_string(),
			conn,
			parent_path: String::new(), // Engine will overwrite this with correct path
		})
	}
}
