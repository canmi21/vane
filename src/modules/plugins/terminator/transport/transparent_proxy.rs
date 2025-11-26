/* src/modules/plugins/terminator/transport/transparent_proxy.rs */

use crate::modules::{
	kv::KvStore,
	plugins::model::{ConnectionObject, ParamDef, ParamType, Plugin, ResolvedInputs, Terminator},
	stack::transport::{model::ResolvedTarget, proxy},
};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use fancy_log::{LogLevel, log};
use serde_json::Value;
use std::any::Any;

/// A built-in Terminator plugin to proxy a connection transparently.
pub struct TransparentProxyPlugin;

impl Plugin for TransparentProxyPlugin {
	fn name(&self) -> &'static str {
		"internal.transport.proxy.transparent"
	}

	fn params(&self) -> Vec<ParamDef> {
		vec![
			ParamDef {
				name: "target.ip",
				required: true,
				param_type: ParamType::String,
			},
			ParamDef {
				name: "target.port",
				required: true,
				param_type: ParamType::Integer,
			},
		]
	}

	fn as_any(&self) -> &dyn Any {
		self
	}
}

#[async_trait]
impl Terminator for TransparentProxyPlugin {
	async fn execute(
		&self,
		inputs: ResolvedInputs,
		kv: &KvStore,
		conn: ConnectionObject,
	) -> Result<()> {
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

		let protocol = kv
			.get("conn.proto")
			.map(|s| s.as_str())
			.unwrap_or("unknown");

		match (protocol, conn) {
			("tcp", ConnectionObject::Tcp(stream)) => {
				proxy::proxy_tcp_stream(stream, target).await?;
			}
			("udp", ConnectionObject::Udp { .. }) => {
				log(
					LogLevel::Debug,
					"⚙ TODO: UDP transparent proxy terminator not yet implemented.",
				);
			}
			(proto, _) => {
				return Err(anyhow!(
					"Protocol mismatch: KvStore says '{}', but received a different ConnectionObject type.",
					proto
				));
			}
		}

		Ok(())
	}
}
