use std::sync::Arc;

use crate::conn_context::ConnId;
use crate::error::SerializedError;
use crate::ir::NodeId;

pub trait FlowLogSink: Send + Sync {
	fn emit(&self, event: FlowLogEvent);
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct FlowLogEvent {
	pub t: u64,
	pub conn: ConnId,
	pub seq: u32,
	pub kind: FlowLogKind,
	pub node: Option<NodeId>,
	pub error: Option<Arc<SerializedError>>,
	pub data: Option<serde_json::Value>,
}

#[derive(Copy, Clone, Eq, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub enum FlowLogKind {
	Check,
	Middleware,
	Fetch,
	Terminate,
	Error,
	SecurityLimit,
	Upgrade,
}
