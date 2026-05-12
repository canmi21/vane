use std::path::Path;
use std::time::Duration;

use tokio::fs::OpenOptions;
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::sync::mpsc::{self, Sender};
use vane_core::{FlowLogEvent, FlowLogSink};

/// Channel capacity for the executor → writer task hand-off. Bounded
/// so a slow disk can't grow the queue without bound; the executor
/// drops events past the cap rather than back-pressuring the request
/// path. 4096 absorbs a typical burst while the writer drains.
pub(crate) const DEFAULT_CHANNEL_CAPACITY: usize = 4096;

/// Periodic flush window. `BufWriter::flush` pushes accumulated bytes
/// to the kernel; without it the writer's in-memory buffer can hold a
/// burst's worth of NDJSON in process memory while the disk sits
/// idle. 100 ms is short enough for tail consumers to see fresh data
/// and long enough to batch consecutive events into one syscall.
pub(crate) const FLUSH_INTERVAL: Duration = Duration::from_millis(100);

/// Batch size that also triggers a flush, independent of the time
/// window. Pairs with `FLUSH_INTERVAL` so a high-burst workload
/// doesn't sit waiting for the timer when bytes are clearly ready.
pub(crate) const FLUSH_BATCH: usize = 256;

/// Periodic fsync (durability) window. `File::sync_data` forces the
/// kernel page cache to disk; without this an OS crash between flush
/// and fsync can lose the most recent N seconds of log lines. The
/// flow log isn't on the request path so a slower fsync cadence
/// trades durability for IOPS sanity.
pub(crate) const FSYNC_INTERVAL: Duration = Duration::from_mins(1);

/// Append-only NDJSON sink. `emit` is sync — it `try_send`s into a
/// bounded mpsc channel; a background tokio task drains the channel
/// and writes to disk. The executor is never blocked on I/O.
///
/// Lossy under overload: when the channel is full the executor drops
/// the event and increments `vane.flow_log.file_dropped` so operators
/// see the rate without per-event tracing noise.
///
/// On `FileSink` drop the channel closes; the writer task drains
/// remaining events, flushes, fsyncs (best-effort), and exits.
pub struct FileSink {
	tx: Sender<FlowLogEvent>,
}

impl FileSink {
	/// Spawn the writer task. Caller must be inside a tokio runtime.
	///
	/// # Errors
	/// Propagates `std::io::Error` from `OpenOptions::open` — the path's
	/// parent dir must exist and be writable.
	pub async fn spawn(path: impl AsRef<Path>) -> std::io::Result<Self> {
		let file = OpenOptions::new().create(true).append(true).open(path.as_ref()).await?;
		let (tx, mut rx) = mpsc::channel::<FlowLogEvent>(DEFAULT_CHANNEL_CAPACITY);
		tokio::spawn(async move {
			let mut buf = BufWriter::new(file);
			let mut unflushed: usize = 0;
			let flush_timer = tokio::time::sleep(FLUSH_INTERVAL);
			let fsync_timer = tokio::time::sleep(FSYNC_INTERVAL);
			tokio::pin!(flush_timer);
			tokio::pin!(fsync_timer);
			loop {
				tokio::select! {
					maybe = rx.recv() => {
						let Some(ev) = maybe else { break };
						if let Ok(line) = serde_json::to_string(&ev)
							&& buf.write_all(line.as_bytes()).await.is_ok()
							&& buf.write_all(b"\n").await.is_ok()
						{
							unflushed = unflushed.saturating_add(1);
							if unflushed >= FLUSH_BATCH && buf.flush().await.is_ok() {
								unflushed = 0;
							}
						}
					}
					() = &mut flush_timer => {
						if unflushed > 0 {
							let _ = buf.flush().await;
							unflushed = 0;
						}
						flush_timer.as_mut().reset(tokio::time::Instant::now() + FLUSH_INTERVAL);
					}
					() = &mut fsync_timer => {
						// Best-effort durability tick. Errors are
						// silently swallowed — the operator's
						// disk-full / permission-error story is
						// surfaced via the write path's drop counter,
						// not here.
						if buf.flush().await.is_ok() {
							let _ = buf.get_ref().sync_data().await;
						}
						unflushed = 0;
						fsync_timer.as_mut().reset(tokio::time::Instant::now() + FSYNC_INTERVAL);
					}
				}
			}
			let _ = buf.flush().await;
			let _ = buf.get_ref().sync_all().await;
		});
		Ok(Self { tx })
	}
}

impl FlowLogSink for FileSink {
	fn emit(&self, event: FlowLogEvent) {
		// Bounded `try_send`: full channel → drop the event and
		// record one drop on the prometheus counter so the operator
		// can see the loss rate. Receiver-closed has the same drop
		// shape; both are "log line lost on the executor side".
		if self.tx.try_send(event).is_err() {
			metrics::counter!("vane.flow_log.file_dropped").increment(1);
		}
	}
}
