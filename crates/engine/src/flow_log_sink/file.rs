use std::path::Path;

use tokio::fs::OpenOptions;
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::sync::mpsc::{self, UnboundedSender};
use vane_core::{FlowLogEvent, FlowLogSink};

/// Append-only NDJSON sink. `emit` is sync — it pushes into an unbounded
/// mpsc channel; a background tokio task drains the channel and writes
/// to disk. The executor is never blocked on I/O.
///
/// On `FileSink` drop the channel closes; the writer task drains
/// remaining events, flushes, and exits.
pub struct FileSink {
	tx: UnboundedSender<FlowLogEvent>,
}

impl FileSink {
	/// Spawn the writer task. Caller must be inside a tokio runtime.
	///
	/// # Errors
	/// Propagates `std::io::Error` from `OpenOptions::open` — the path's
	/// parent dir must exist and be writable.
	pub async fn spawn(path: impl AsRef<Path>) -> std::io::Result<Self> {
		let file = OpenOptions::new().create(true).append(true).open(path.as_ref()).await?;
		let (tx, mut rx) = mpsc::unbounded_channel::<FlowLogEvent>();
		tokio::spawn(async move {
			let mut buf = BufWriter::new(file);
			while let Some(ev) = rx.recv().await {
				if let Ok(line) = serde_json::to_string(&ev) {
					if buf.write_all(line.as_bytes()).await.is_err() {
						break;
					}
					if buf.write_all(b"\n").await.is_err() {
						break;
					}
					if buf.flush().await.is_err() {
						break;
					}
				}
			}
			let _ = buf.flush().await;
		});
		Ok(Self { tx })
	}
}

impl FlowLogSink for FileSink {
	fn emit(&self, event: FlowLogEvent) {
		// Receiver-closed → silently drop. Don't panic on log-write
		// failure; the daemon must not crash because the operator
		// removed the log file.
		let _ = self.tx.send(event);
	}
}
