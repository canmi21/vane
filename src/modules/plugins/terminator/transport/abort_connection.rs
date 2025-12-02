/* src/modules/plugins/terminator/transport/abort_connection.rs */

use crate::modules::{
	kv::KvStore,
	plugins::model::{ConnectionObject, ParamDef, Plugin, ResolvedInputs, Terminator},
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
	async fn execute(
		&self,
		_inputs: ResolvedInputs,
		_kv: &KvStore,
		conn: ConnectionObject,
	) -> Result<()> {
		log(LogLevel::Debug, "➜ Aborting connection intentionally...");

		match conn {
			ConnectionObject::Tcp(mut stream) => {
				// We attempt to shutdown cleanly, but if it fails (e.g. already closed),
				// we don't treat it as a plugin error.
				if let Err(e) = stream.shutdown().await {
					log(
						LogLevel::Debug,
						&format!("⚙ TCP shutdown error (likely harmless): {}", e),
					);
				}
			}
			ConnectionObject::Udp { .. } => {
				// UDP is connectionless; dropping the ConnectionObject implies
				// no further packets are sent/received for this flow context.
				log(LogLevel::Debug, "⚙ UDP flow dropped.");
			}
		}

		// Return Ok(()) so the engine logs "✓ Flow terminated successfully"
		Ok(())
	}
}
