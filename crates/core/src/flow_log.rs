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

#[cfg(test)]
mod tests {
	use parking_lot::Mutex;

	use super::*;
	use crate::error::Error;

	struct RecordingSink {
		events: Mutex<Vec<FlowLogEvent>>,
	}

	impl FlowLogSink for RecordingSink {
		fn emit(&self, event: FlowLogEvent) {
			self.events.lock().push(event);
		}
	}

	fn sample_event(seq: u32, kind: FlowLogKind) -> FlowLogEvent {
		FlowLogEvent {
			t: 1_234_567_890_123,
			conn: ConnId(0x0bad_f00d_dead_beef),
			seq,
			kind,
			node: Some(NodeId::new(42)),
			error: None,
			data: Some(serde_json::json!({ "kv": "v" })),
		}
	}

	#[test]
	fn flow_log_event_round_trips_through_json() {
		let err = Error::internal("boom");
		let event = FlowLogEvent {
			t: 1_700_000_000_000,
			conn: ConnId(7),
			seq: 13,
			kind: FlowLogKind::Error,
			node: Some(NodeId::new(3)),
			error: Some(Arc::new(SerializedError::from(&err))),
			data: Some(serde_json::json!({ "note": "sample" })),
		};
		let encoded = serde_json::to_string(&event).expect("serialize");
		let decoded: FlowLogEvent = serde_json::from_str(&encoded).expect("deserialize");

		assert_eq!(decoded.t, event.t);
		assert_eq!(decoded.conn, event.conn);
		assert_eq!(decoded.seq, event.seq);
		assert_eq!(decoded.kind, event.kind);
		assert_eq!(decoded.node, event.node);
		assert_eq!(decoded.data, event.data);
		let dec_err = decoded.error.as_ref().expect("error preserved");
		let src_err = event.error.as_ref().expect("error set");
		assert_eq!(dec_err.kind, src_err.kind);
		assert_eq!(dec_err.reason, src_err.reason);
		assert_eq!(dec_err.message, src_err.message);
		assert_eq!(dec_err.ctx, src_err.ctx);
		assert_eq!(dec_err.source_chain, src_err.source_chain);
		assert_eq!(dec_err.http_status, src_err.http_status);
		assert_eq!(dec_err.retryable, src_err.retryable);
	}

	#[test]
	fn flow_log_kind_serde_round_trip_per_variant() {
		for k in [
			FlowLogKind::Check,
			FlowLogKind::Middleware,
			FlowLogKind::Fetch,
			FlowLogKind::Terminate,
			FlowLogKind::Error,
			FlowLogKind::SecurityLimit,
			FlowLogKind::Upgrade,
		] {
			let encoded = serde_json::to_string(&k).expect("serialize");
			let decoded: FlowLogKind = serde_json::from_str(&encoded).expect("deserialize");
			assert_eq!(decoded, k);
		}
	}

	#[test]
	fn flow_log_sink_trait_accepts_concrete_impl_and_records_in_order() {
		let sink = RecordingSink { events: Mutex::new(Vec::new()) };
		let first = sample_event(1, FlowLogKind::Check);
		let second = sample_event(2, FlowLogKind::Middleware);
		sink.emit(first.clone());
		sink.emit(second.clone());
		let recorded = sink.events.lock();
		assert_eq!(recorded.len(), 2);
		assert_eq!(recorded[0].seq, first.seq);
		assert_eq!(recorded[0].kind, first.kind);
		assert_eq!(recorded[1].seq, second.seq);
		assert_eq!(recorded[1].kind, second.kind);
	}

	#[test]
	fn flow_log_sink_is_usable_as_trait_object() {
		let sink = RecordingSink { events: Mutex::new(Vec::new()) };
		// Coerce to trait object and invoke through the vtable; validates
		// that the trait's `fn emit(&self, ...)` signature is object-safe.
		let dyn_sink: &dyn FlowLogSink = &sink;
		dyn_sink.emit(sample_event(1, FlowLogKind::Fetch));
		assert_eq!(sink.events.lock().len(), 1);
	}
}
