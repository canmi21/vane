/* src/layers/l4/flow.rs */

use anyhow::Result;

use crate::engine::context::TransportContext;
use crate::engine::contract::{ConnectionObject, ProcessingStep, TerminatorResult};
use crate::engine::executor;
use crate::resources::kv::KvStore;

use bytes::Bytes;

/// Public entry point for executing a flow.
pub async fn execute(
	step: &ProcessingStep,
	kv: &mut KvStore,
	conn: ConnectionObject,
	initial_payloads: ahash::AHashMap<String, Bytes>,
) -> Result<TerminatorResult> {
	kv.insert("conn.layer".to_string(), "l4".to_string());

	let mut context = TransportContext {
		kv,
		payloads: initial_payloads,
	};
	executor::execute(step, &mut context, conn, String::new()).await
}
