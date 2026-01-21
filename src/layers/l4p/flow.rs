/* src/layers/l4p/flow.rs */

use anyhow::Result;

use crate::engine::context::TransportContext;
use crate::engine::executor;
use crate::engine::interfaces::{ConnectionObject, ProcessingStep, TerminatorResult};
use crate::resources::kv::KvStore;

use bytes::Bytes;

pub async fn execute(
	step: &ProcessingStep,
	kv: &mut KvStore,
	conn: ConnectionObject,
	parent_path: String,
	initial_payloads: ahash::AHashMap<String, Bytes>,
) -> Result<TerminatorResult> {
	kv.insert("conn.layer".to_owned(), "l4p".to_owned());

	let mut context = TransportContext {
		kv,
		payloads: initial_payloads,
	};
	executor::execute(step, &mut context, conn, parent_path).await
}
