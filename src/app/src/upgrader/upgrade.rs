use anyhow::{Result, anyhow};
use async_trait::async_trait;
use fancy_log::{LogLevel, log};
use serde_json::Value;
use std::any::Any;
use vane_engine::engine::interfaces::{
	ConnectionObject, Layer, ParamDef, ParamType, Plugin, ResolvedInputs, Terminator,
	TerminatorResult,
};
use vane_primitives::kv::KvStore;

pub struct UpgradePlugin;

impl Plugin for UpgradePlugin {
	fn name(&self) -> &'static str {
		"internal.transport.upgrade"
	}

	fn params(&self) -> Vec<ParamDef> {
		vec![
			ParamDef { name: "protocol".into(), required: true, param_type: ParamType::String },
			ParamDef { name: "cert".into(), required: false, param_type: ParamType::String },
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
impl Terminator for UpgradePlugin {
	fn supported_layers(&self) -> Vec<Layer> {
		vec![Layer::L4, Layer::L4Plus]
	}

	async fn execute(
		&self,
		inputs: ResolvedInputs,
		kv: &mut KvStore,
		conn: ConnectionObject,
	) -> Result<TerminatorResult> {
		let protocol = inputs
			.get("protocol")
			.and_then(Value::as_str)
			.ok_or_else(|| anyhow!("Resolved input 'protocol' is missing or not a string"))?;

		// Handle optional Certificate SNI override ('cert')
		if let Some(cert_sni) = inputs.get("cert").and_then(Value::as_str) {
			match conn {
				ConnectionObject::Tcp(_) | ConnectionObject::Udp { .. } => {
					log(
						LogLevel::Warn,
						&format!("⚠ Ignored 'cert' parameter for L4 -> L4+ upgrade to '{protocol}'."),
					);
				}
				ConnectionObject::Stream(_) => {
					log(
						LogLevel::Debug,
						&format!("⚙ Upgrade requested with explicit cert override: {cert_sni}"),
					);
					kv.insert("tls.termination.cert_sni".to_owned(), cert_sni.to_owned());
				}
				ConnectionObject::Virtual(_) => {
					// Virtual connections in L7 might support this if re-encrypting
				}
			}
		}

		match (&conn, protocol) {
			(ConnectionObject::Tcp(_), "tls" | "http")
			| (ConnectionObject::Udp { .. }, "quic" | "h3" | "httpx")
			| (ConnectionObject::Stream(_), _) => {}

			(ConnectionObject::Tcp(_), "quic") => return Err(anyhow!("Invalid Upgrade: TCP -> QUIC")),
			(ConnectionObject::Virtual(_), _) => {
				log(
					LogLevel::Warn,
					&format!("⚠ Attempting upgrade on Virtual connection to '{protocol}'."),
				);
			}
			_ => log(LogLevel::Warn, &format!("⚠ Allowing unchecked upgrade to '{protocol}'.")),
		}

		log(LogLevel::Debug, &format!("➜ Signal upgrade to protocol: {protocol}"));

		Ok(TerminatorResult::Upgrade {
			protocol: protocol.to_owned(),
			conn,
			parent_path: String::new(),
		})
	}
}
