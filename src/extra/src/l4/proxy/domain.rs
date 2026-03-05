/* src/plugins/l4/proxy/domain.rs */

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
use vane_engine::shared::resolver;
use vane_primitives::kv::KvStore;
use vane_primitives::model::ResolvedTarget;

/// A built-in Terminator plugin to proxy a connection to a domain.
pub struct ProxyDomainPlugin;

impl Plugin for ProxyDomainPlugin {
	fn name(&self) -> &'static str {
		"internal.transport.proxy.domain"
	}

	fn params(&self) -> Vec<ParamDef> {
		vec![
			ParamDef { name: "target.domain".into(), required: true, param_type: ParamType::String },
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
impl Terminator for ProxyDomainPlugin {
	fn supported_layers(&self) -> Vec<Layer> {
		vec![Layer::L4, Layer::L4Plus]
	}

	async fn execute(
		&self,
		inputs: ResolvedInputs,
		kv: &mut KvStore,
		conn: ConnectionObject,
	) -> Result<TerminatorResult> {
		let target_domain = inputs
			.get("target.domain")
			.and_then(Value::as_str)
			.ok_or_else(|| anyhow!("Resolved input 'target.domain' is missing or not a string"))?;

		let target_port = inputs
			.get("target.port")
			.and_then(Value::as_u64)
			.map(|p| p as u16)
			.ok_or_else(|| anyhow!("Resolved input 'target.port' is missing or not an integer"))?;

		let ips = resolver::resolve_domain_to_ips(target_domain).await;

		if ips.is_empty() {
			return Err(anyhow!("DNS resolution failed: No IPs found for domain '{target_domain}'"));
		}

		let selected_ip = if ips.len() == 1 {
			ips[0]
		} else {
			let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().subsec_nanos();
			let index = (nanos as usize) % ips.len();
			ips[index]
		};

		let target = ResolvedTarget { ip: selected_ip.to_string(), port: target_port };

		execute_proxy(target, kv, conn).await?;
		Ok(TerminatorResult::Finished)
	}
}
