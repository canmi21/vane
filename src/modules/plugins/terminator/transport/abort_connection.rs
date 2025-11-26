/* src/modules/plugins/terminator/transport/abort_connection.rs */

use crate::modules::{
	kv::KvStore,
	plugins::model::{ConnectionObject, ParamDef, Plugin, ResolvedInputs, Terminator},
};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
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
}

#[async_trait]
impl Terminator for AbortConnectionPlugin {
	async fn execute(
		&self,
		_inputs: ResolvedInputs,
		_kv: &KvStore,
		conn: ConnectionObject,
	) -> Result<()> {
		match conn {
			ConnectionObject::Tcp(mut stream) => {
				let _ = stream.shutdown().await;
			}
			ConnectionObject::Udp { .. } => {}
		}
		Err(anyhow!("Flow aborted by internal.transport.abort"))
	}
}
