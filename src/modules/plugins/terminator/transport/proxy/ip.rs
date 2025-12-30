/* src/modules/plugins/terminator/transport/proxy/ip.rs */

use super::execute_proxy;
use crate::modules::{
	kv::KvStore,
	plugins::model::{
		ConnectionObject, Layer, ParamDef, ParamType, Plugin, ResolvedInputs, Terminator,
		TerminatorResult,
	},
	stack::transport::model::ResolvedTarget,
};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde_json::Value;
use std::any::Any;
use std::borrow::Cow;

/// A built-in Terminator plugin to proxy a connection transparently using explicit IP and Port.
pub struct TransparentProxyPlugin;

impl Plugin for TransparentProxyPlugin {
	fn name(&self) -> &'static str {
		"internal.transport.proxy"
	}

	fn params(&self) -> Vec<ParamDef> {
		vec![
			ParamDef {
				name: "target.ip".into(),
				required: true,
				param_type: ParamType::String,
			},
			ParamDef {
				name: "target.port".into(),
				required: true,
				param_type: ParamType::Integer,
			},
		]
	}

	fn supported_protocols(&self) -> Vec<Cow<'static, str>> {
		vec!["tcp".into(), "udp".into()]
	}

	fn as_any(&self) -> &dyn Any {
		self
	}

	fn as_terminator(&self) -> Option<&dyn Terminator> {
		Some(self)
	}
}

#[async_trait]
impl Terminator for TransparentProxyPlugin {
	fn supported_layers(&self) -> Vec<Layer> {
		vec![Layer::L4, Layer::L4Plus]
	}

	async fn execute(
		&self,
		inputs: ResolvedInputs,
		kv: &mut KvStore,
		conn: ConnectionObject,
	) -> Result<TerminatorResult> {
		let target_ip = inputs
			.get("target.ip")
			.and_then(Value::as_str)
			.ok_or_else(|| anyhow!("Resolved input 'target.ip' is missing or not a string"))?;

		let target_port = inputs
			.get("target.port")
			.and_then(Value::as_u64)
			.map(|p| p as u16)
			.ok_or_else(|| anyhow!("Resolved input 'target.port' is missing or not an integer"))?;

		let target = ResolvedTarget {
			ip: target_ip.to_string(),
			port: target_port,
		};

		execute_proxy(target, kv, conn).await?;
		Ok(TerminatorResult::Finished)
	}
}
