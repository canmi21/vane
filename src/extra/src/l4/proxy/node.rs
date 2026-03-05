/* src/plugins/l4/proxy/node.rs */

use super::execute_proxy;
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde_json::Value;
use std::any::Any;
use std::time::{SystemTime, UNIX_EPOCH};
use vane_engine::engine::interfaces::{
	ConnectionObject, Layer, ParamDef, ParamType, Plugin, ResolvedInputs, Terminator,
	TerminatorResult,
};
use vane_primitives::kv::KvStore;
use vane_primitives::model::ResolvedTarget;

/// A built-in Terminator plugin to proxy a connection to a specific Node.
pub struct ProxyNodePlugin;

impl Plugin for ProxyNodePlugin {
	fn name(&self) -> &'static str {
		"internal.transport.proxy.node"
	}

	fn params(&self) -> Vec<ParamDef> {
		vec![
			ParamDef { name: "target.node".into(), required: true, param_type: ParamType::String },
			ParamDef { name: "target.port".into(), required: true, param_type: ParamType::Integer },
		]
	}

	fn as_any(&self) -> &dyn Any {
		self
	}

	fn as_terminator(&self) -> Option<&dyn Terminator> {
		Some(self)
	}
}

#[async_trait]
impl Terminator for ProxyNodePlugin {
	fn supported_layers(&self) -> Vec<Layer> {
		vec![Layer::L4, Layer::L4Plus]
	}

	async fn execute(
		&self,
		inputs: ResolvedInputs,
		kv: &mut KvStore,
		conn: ConnectionObject,
	) -> Result<TerminatorResult> {
		let target_node_name = inputs
			.get("target.node")
			.and_then(Value::as_str)
			.ok_or_else(|| anyhow!("Resolved input 'target.node' is missing or not a string"))?;

		let target_port = inputs
			.get("target.port")
			.and_then(Value::as_u64)
			.map(|p| p as u16)
			.ok_or_else(|| anyhow!("Resolved input 'target.port' is missing or not an integer"))?;

		let config_manager = vane_engine::config::get();
		let nodes_config = config_manager
			.nodes
			.get()
			.unwrap_or_else(|| std::sync::Arc::new(vane_engine::config::NodesConfig::default()));
		let candidates: Vec<&String> = nodes_config
			.processed
			.iter()
			.filter(|n| n.node_name == target_node_name && n.port == target_port)
			.map(|n| &n.address)
			.collect();

		if candidates.is_empty() {
			return Err(anyhow!(
				"No available IP addresses found for node '{target_node_name}' on port {target_port}"
			));
		}

		let selected_ip = if candidates.len() == 1 {
			candidates[0]
		} else {
			let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().subsec_nanos();
			let index = (nanos as usize) % candidates.len();
			candidates[index]
		};

		let target = ResolvedTarget { ip: selected_ip.clone(), port: target_port };

		execute_proxy(target, kv, conn).await?;
		Ok(TerminatorResult::Finished)
	}
}
