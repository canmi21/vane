use tokio_util::sync::CancellationToken;

use crate::flow_log::FlowLogSink;

pub struct FlowCtx<'a> {
	pub span: &'a mut tracing::Span,
	pub log: &'a mut dyn FlowLogSink,
	pub cancel: &'a CancellationToken,
}
