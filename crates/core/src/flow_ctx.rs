use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::flow_log::{FlowLogSink, FlowLogVerbosity, TrajectoryBuilder};

/// Per-walk execution context. Constructed once per L4 connection (and
/// re-constructed per L7 request when a hyper service-fn dispatches into
/// the L7 sub-graph). Fields are *owned* — no lifetime parameter — so the
/// struct survives `tokio::spawn` and `move` closures (notably hyper's
/// service-fn closure at `Node::Upgrade`, which captures `log` / `cancel`
/// / `verbosity` per request).
///
/// `Arc<dyn FlowLogSink>` and `CancellationToken` clone cheaply (each is
/// internally an `Arc`), and `tracing::Span` is also `Arc`-backed; the
/// per-request clones in the hyper bridge are O(1).
pub struct FlowCtx {
	pub span: tracing::Span,
	pub log: Arc<dyn FlowLogSink>,
	pub cancel: CancellationToken,
	/// Verbosity selected when this connection was accepted. The listener
	/// reads `engine::VerbosityState` once at `FlowCtx` construction;
	/// in-flight connections retain the value they were built with.
	pub verbosity: FlowLogVerbosity,
	/// Walker-internal step accumulator. The executor pushes one entry
	/// per node-visit and emits a single `FlowLogKind::Trajectory` event
	/// from `finalize()` at terminate or error.
	pub trajectory: TrajectoryBuilder,
}

#[cfg(test)]
mod tests {
	use parking_lot::Mutex;

	use super::*;
	use crate::conn_context::ConnId;
	use crate::flow_log::FlowLogEvent;
	use crate::ir::NodeId;

	struct NullSink {
		count: Mutex<u32>,
	}

	impl FlowLogSink for NullSink {
		fn emit(&self, _event: FlowLogEvent) {
			*self.count.lock() += 1;
		}
	}

	// Compile-gate: a FlowCtx must be constructible from a concrete sink
	// wrapped in `Arc<dyn FlowLogSink>`, alongside an owned tracing::Span
	// and CancellationToken, plus the verbosity / trajectory fields the
	// walker reads. Field visibility regressions break this.
	#[test]
	fn flow_ctx_accepts_arc_dyn_sink_and_owned_fields() {
		let sink: Arc<dyn FlowLogSink> = Arc::new(NullSink { count: Mutex::new(0) });
		let span = tracing::Span::none();
		let cancel = CancellationToken::new();
		let ctx = FlowCtx {
			span,
			log: sink,
			cancel,
			verbosity: FlowLogVerbosity::Trajectory,
			trajectory: TrajectoryBuilder::new(ConnId(0), NodeId::new(0), 0),
		};
		let _ = &ctx.span;
		let _ = &ctx.log;
		let _ = &ctx.cancel;
		let _ = &ctx.verbosity;
		let _ = &ctx.trajectory;
	}
}
