/* src/modules/stack/carrier/flow.rs */

use anyhow::Result;

use crate::modules::{
	flow::{context::TransportContext, engine},
	kv::KvStore,
	plugins::core::model::{ConnectionObject, ProcessingStep, TerminatorResult},
};

use bytes::Bytes;
use std::collections::HashMap;

pub async fn execute(
	step: &ProcessingStep,
	kv: &mut KvStore,
	conn: ConnectionObject,
	parent_path: String,
	initial_payloads: HashMap<String, Bytes>,
) -> Result<TerminatorResult> {
	kv.insert("conn.layer".to_string(), "l4p".to_string());

	let mut context = TransportContext {
		kv,
		payloads: initial_payloads,
	};
	engine::execute(step, &mut context, conn, parent_path).await
}
