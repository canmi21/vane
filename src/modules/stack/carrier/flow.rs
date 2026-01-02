/* src/modules/stack/carrier/flow.rs */

use anyhow::Result;

use crate::modules::{
	flow::{context::TransportContext, engine},
	kv::KvStore,
	plugins::core::model::{ConnectionObject, ProcessingStep, TerminatorResult},
};

pub async fn execute(
	step: &ProcessingStep,
	kv: &mut KvStore,
	conn: ConnectionObject,
	parent_path: String,
) -> Result<TerminatorResult> {
	kv.insert("conn.layer".to_string(), "l4p".to_string());

	let mut context = TransportContext { kv };
	engine::execute(step, &mut context, conn, parent_path).await
}
