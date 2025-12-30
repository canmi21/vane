/* src/modules/stack/transport/flow.rs */

use anyhow::Result;

use crate::modules::{
	flow::{context::TransportContext, engine},
	kv::KvStore,
	plugins::model::{ConnectionObject, ProcessingStep, TerminatorResult},
};

/// Public entry point for executing a flow.
pub async fn execute(
	step: &ProcessingStep,
	kv: &mut KvStore,
	conn: ConnectionObject,
) -> Result<TerminatorResult> {
	kv.insert("conn.layer".to_string(), "l4".to_string());

	let mut context = TransportContext { kv };
	engine::execute(step, &mut context, conn, String::new()).await
}
