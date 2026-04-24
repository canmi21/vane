use tokio_util::sync::CancellationToken;

use crate::flow_log::FlowLogSink;

pub struct FlowCtx<'a> {
	pub span: &'a mut tracing::Span,
	pub log: &'a mut dyn FlowLogSink,
	pub cancel: &'a CancellationToken,
}

#[cfg(test)]
mod tests {
	use parking_lot::Mutex;

	use super::*;
	use crate::flow_log::FlowLogEvent;

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
	// and CancellationToken. Field visibility regressions break this.
	#[test]
	fn flow_ctx_accepts_dyn_sink_and_borrowed_fields() {
		let mut sink = NullSink { count: Mutex::new(0) };
		let mut span = tracing::Span::none();
		let cancel = CancellationToken::new();
		let ctx = FlowCtx { span: &mut span, log: &mut sink as &mut dyn FlowLogSink, cancel: &cancel };
		// Touch each field so borrow-checking the construction is exercised.
		let _ = &ctx.span;
		let _ = &ctx.log;
		let _ = &ctx.cancel;
	}
}
