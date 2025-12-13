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

/// A built-in Terminator plugin to upgrade the connection protocol.
/// This transfers control from the current layer (e.g., L4) to a higher layer resolver (e.g., L4+ TLS).
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
		// Can be used at L4 (to upgrade to L4+ TLS) or L4+ (to upgrade to L7 HTTP)
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

		// We MUST return the connection object back to the engine!
		Ok(TerminatorResult::Upgrade {
			protocol: protocol.to_string(),
			conn,
		})
	}
}
