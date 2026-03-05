/* src/layers/l4p/flow.rs */

use anyhow::Result;

use vane_engine::engine::context::TransportContext;
use vane_engine::engine::executor;
use vane_engine::engine::interfaces::{ConnectionObject, ProcessingStep, TerminatorResult};
use vane_primitives::kv::KvStore;

use bytes::Bytes;

pub async fn execute(
	step: &ProcessingStep,
	kv: &mut KvStore,
	conn: ConnectionObject,
	parent_path: String,
	initial_payloads: ahash::AHashMap<String, Bytes>,
) -> Result<TerminatorResult> {
	kv.insert("conn.layer".to_owned(), "l4p".to_owned());

	let mut context = TransportContext { kv, payloads: initial_payloads };
	executor::execute(step, &mut context, conn, parent_path).await
}
