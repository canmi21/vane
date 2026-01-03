/* src/modules/plugins/l7/cgi/stream.rs */

use crate::common::lifecycle::{Error, Result as VaneResult};
use bytes::Bytes;
use fancy_log::{LogLevel, log};
use http_body::{Body, Frame, SizeHint};
use std::{
	pin::Pin,
	task::{Context, Poll},
	time::Duration,
};
use tokio::{io::AsyncReadExt, process::ChildStdout, sync::mpsc, time::timeout};

pub struct CgiResponseBody {
	rx: mpsc::Receiver<VaneResult<Bytes>>,
}

impl CgiResponseBody {
	pub fn new(rx: mpsc::Receiver<VaneResult<Bytes>>) -> Self {
		Self { rx }
	}
}

impl Body for CgiResponseBody {
	type Data = Bytes;
	type Error = Error;

	fn poll_frame(
		mut self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Option<std::result::Result<Frame<Self::Data>, Self::Error>>> {
		match self.rx.poll_recv(cx) {
			Poll::Ready(Some(Ok(bytes))) => Poll::Ready(Some(Ok(Frame::data(bytes)))),
			Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(e))),
			Poll::Ready(None) => Poll::Ready(None),
			Poll::Pending => Poll::Pending,
		}
	}

	fn is_end_stream(&self) -> bool {
		false
	}

	fn size_hint(&self) -> SizeHint {
		// Do NOT return with_exact(0) here.
		// That tells Hyper the body is empty, causing it to drop the stream.
		// Default SizeHint implies unknown size, triggering chunked encoding.
		SizeHint::default()
	}
}

pub async fn pump_stdout(
	mut stdout: ChildStdout,
	tx: mpsc::Sender<VaneResult<Bytes>>,
	initial_chunk: Bytes,
	max_size: usize,
	timeout_sec: u64,
) {
	let mut buf = [0u8; 8192];
	let mut total_bytes = initial_chunk.len();

	if !initial_chunk.is_empty() {
		if tx.send(Ok(initial_chunk)).await.is_err() {
			return;
		}
	}

	loop {
		let read_future = stdout.read(&mut buf);
		match timeout(Duration::from_secs(timeout_sec), read_future).await {
			Ok(Ok(0)) => {
				log(
					LogLevel::Debug,
					&format!("✓ CGI Body Pump EOF. Total: {} bytes", total_bytes),
				);
				break;
			}
			Ok(Ok(n)) => {
				total_bytes += n;
				if total_bytes > max_size {
					log(LogLevel::Error, "CGI Body Exceeded Max Size.");
					let _ = tx
						.send(Err(Error::System("CGI Body Exceeded Max Size".into())))
						.await;
					return;
				}

				let data = Bytes::copy_from_slice(&buf[..n]);
				if tx.send(Ok(data)).await.is_err() {
					break;
				}
			}
			Ok(Err(e)) => {
				log(LogLevel::Error, &format!("CGI Read Error: {}", e));
				let _ = tx.send(Err(Error::System(e.to_string()))).await;
				break;
			}
			Err(_) => {
				log(LogLevel::Error, "CGI Body Idle Timeout.");
				let _ = tx
					.send(Err(Error::System("CGI Body Idle Timeout".into())))
					.await;
				return;
			}
		}
	}
}
