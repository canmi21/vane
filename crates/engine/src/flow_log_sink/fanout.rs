use std::sync::Arc;

use vane_core::{FlowLogEvent, FlowLogSink};

pub struct FanoutSink {
	sinks: Vec<Arc<dyn FlowLogSink>>,
}

impl FanoutSink {
	#[must_use]
	pub fn new(sinks: Vec<Arc<dyn FlowLogSink>>) -> Self {
		Self { sinks }
	}

	#[must_use]
	pub fn len(&self) -> usize {
		self.sinks.len()
	}

	#[must_use]
	pub fn is_empty(&self) -> bool {
		self.sinks.is_empty()
	}
}

impl FlowLogSink for FanoutSink {
	fn emit(&self, event: FlowLogEvent) {
		// Each `emit` is required to be cheap; we clone the event once per
		// sink rather than holding any cross-sink lock. This keeps a slow
		// sink (e.g. the FileSink waiting on disk) from blocking ring-
		// buffer reads.
		for s in &self.sinks {
			s.emit(event.clone());
		}
	}
}
