/* src/modules/plugins/terminator/transport/abort.rs */

use crate::modules::{
	kv::KvStore,
	plugins::model::{
		ConnectionObject, Layer, ParamDef, Plugin, ResolvedInputs, Terminator, TerminatorResult,
	},
};
use anyhow::Result;
use async_trait::async_trait;
use fancy_log::{LogLevel, log};
use std::any::Any;
use tokio::io::AsyncWriteExt;

/// A built-in Terminator plugin to immediately close a connection.
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
		// Abort is universally applicable
		vec![Layer::L4, Layer::L4Plus, Layer::L7]
	}

	async fn execute(
		&self,
		_inputs: ResolvedInputs,
		_kv: &KvStore,
		conn: ConnectionObject,
	) -> Result<TerminatorResult> {
		log(LogLevel::Debug, "➜ Aborting connection intentionally...");

		match conn {
			ConnectionObject::Tcp(mut stream) => {
				if let Err(e) = stream.shutdown().await {
					log(
						LogLevel::Debug,
						&format!("⚙ TCP shutdown error (likely harmless): {}", e),
					);
				}
			}
			ConnectionObject::Stream(mut stream) => {
				// Encrypted/Generic stream shutdown
				if let Err(e) = stream.shutdown().await {
					log(
						LogLevel::Debug,
						&format!("⚙ Stream shutdown error (likely harmless): {}", e),
					);
				}
			}
			ConnectionObject::Udp { .. } => {
				log(LogLevel::Debug, "⚙ UDP flow dropped.");
			}
		}

		Ok(TerminatorResult::Finished)
	}
}
