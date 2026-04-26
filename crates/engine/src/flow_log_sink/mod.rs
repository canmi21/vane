mod broadcast;
mod fanout;
mod file;
mod ring_buffer;

use std::sync::Arc;

use vane_core::FlowLogSink;

pub use broadcast::BroadcastSink;
pub use fanout::FanoutSink;
pub use file::FileSink;
pub use ring_buffer::RingBufferSink;

/// Compose the daemon's default `FlowLogSink`:
///
/// - always: an in-memory [`RingBufferSink`] (`10_000` entries / 60s TTL)
/// - if `VANE_FLOW_LOG_FILE=<path>` is set in the environment: also append
///   NDJSON to that path via a [`FileSink`]
///
/// Caller must be inside a tokio runtime context — `FileSink` spawns a
/// writer task. The returned `Arc<dyn FlowLogSink>` is shared across
/// listeners.
///
/// # Errors
/// Propagates the `std::io::Error` from opening the file path when
/// `VANE_FLOW_LOG_FILE` is set but the path is unwritable.
pub async fn default_sink_from_env() -> std::io::Result<Arc<dyn FlowLogSink>> {
	let ring: Arc<dyn FlowLogSink> = Arc::new(RingBufferSink::with_defaults());
	match std::env::var("VANE_FLOW_LOG_FILE") {
		Ok(path) if !path.is_empty() => {
			let file: Arc<dyn FlowLogSink> = Arc::new(FileSink::spawn(path).await?);
			Ok(Arc::new(FanoutSink::new(vec![ring, file])))
		}
		_ => Ok(ring),
	}
}
