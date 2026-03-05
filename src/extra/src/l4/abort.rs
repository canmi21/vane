/* src/extra/src/l4/abort.rs */

use anyhow::Result;
use async_trait::async_trait;
use fancy_log::{LogLevel, log};
use std::any::Any;
use tokio::io::AsyncWriteExt;
use vane_engine::engine::interfaces::{
	ConnectionObject, Layer, ParamDef, Plugin, ResolvedInputs, Terminator, TerminatorResult,
};
use vane_primitives::kv::KvStore;

pub struct AbortConnectionPlugin;

impl Plugin for AbortConnectionPlugin {
	fn name(&self) -> &'static str {
		"internal.transport.abort"
	}

	fn params(&self) -> Vec<ParamDef> {
		vec![]
	}

	fn as_any(&self) -> &dyn Any {
		self
	}

	fn as_terminator(&self) -> Option<&dyn Terminator> {
		Some(self)
	}
}

#[async_trait]
impl Terminator for AbortConnectionPlugin {
	fn supported_layers(&self) -> Vec<Layer> {
		vec![Layer::L4, Layer::L4Plus, Layer::L7]
	}

	async fn execute(
		&self,
		_inputs: ResolvedInputs,
		_kv: &mut KvStore,
		conn: ConnectionObject,
	) -> Result<TerminatorResult> {
		log(LogLevel::Debug, "➜ Aborting connection intentionally...");

		match conn {
			ConnectionObject::Tcp(mut stream) => {
				let _ = stream.shutdown().await;
			}
			ConnectionObject::Stream(mut stream) => {
				let _ = stream.shutdown().await;
			}
			ConnectionObject::Udp { .. } => {
				log(LogLevel::Debug, "⚙ UDP flow dropped.");
			}
			ConnectionObject::Virtual(_) => {
				log(LogLevel::Debug, "⚙ Virtual flow aborted.");
			}
		}

		Ok(TerminatorResult::Finished)
	}
}
