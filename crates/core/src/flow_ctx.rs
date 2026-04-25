use tokio_util::sync::CancellationToken;

use crate::flow_log::{FlowLogSink, FlowLogVerbosity, TrajectoryBuilder};

pub struct FlowCtx<'a> {
	pub span: &'a mut tracing::Span,
	pub log: &'a mut dyn FlowLogSink,
	pub cancel: &'a CancellationToken,
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
	// coerced to `&mut dyn FlowLogSink`, alongside a borrowed tracing::Span
	// and CancellationToken, plus the verbosity / trajectory fields the
	// walker reads. Field visibility regressions break this.
	#[test]
	fn flow_ctx_accepts_dyn_sink_and_borrowed_fields() {
		let mut sink = NullSink { count: Mutex::new(0) };
		let mut span = tracing::Span::none();
		let cancel = CancellationToken::new();
		let ctx = FlowCtx {
			span: &mut span,
			log: &mut sink as &mut dyn FlowLogSink,
			cancel: &cancel,
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
