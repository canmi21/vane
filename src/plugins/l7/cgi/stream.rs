/* src/plugins/l7/cgi/stream.rs */

use crate::common::sys::lifecycle::{Error, Result as VaneResult};
use crate::layers::l7::container::{self, BufferGuard};
use bytes::Bytes;
use fancy_log::{LogLevel, log};
use http_body::{Body, Frame, SizeHint};
use std::{
	pin::Pin,
	task::{Context, Poll},
	time::Duration,
};
use tokio::{io::AsyncReadExt, process::ChildStdout, sync::mpsc, time::timeout};

/// A wrapper for Bytes that carries a memory quota guard.
pub struct QuotaBytes {
	pub data: Bytes,
	pub _guard: BufferGuard,
}

impl QuotaBytes {
	pub fn new(data: Bytes) -> VaneResult<Self> {
		let len = data.len();
		if !container::try_reserve_buffer_memory(len) {
			return Err(Error::System(
				"Global L7 memory limit exceeded for CGI stream buffering.".into(),
			));
		}
		Ok(Self {
			data,
			_guard: BufferGuard::new(len),
		})
	}
}

pub struct CgiResponseBody {
	rx: mpsc::Receiver<VaneResult<QuotaBytes>>,
}

impl CgiResponseBody {
	#[must_use]
	pub fn new(rx: mpsc::Receiver<VaneResult<QuotaBytes>>) -> Self {
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
			Poll::Ready(Some(Ok(quota_bytes))) => Poll::Ready(Some(Ok(Frame::data(quota_bytes.data)))),
			Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(e))),
			Poll::Ready(None) => Poll::Ready(None),
			Poll::Pending => Poll::Pending,
		}
	}

	fn is_end_stream(&self) -> bool {
		false
	}

	fn size_hint(&self) -> SizeHint {
		SizeHint::default()
	}
}

pub async fn pump_stdout(
	mut stdout: ChildStdout,
	tx: mpsc::Sender<VaneResult<QuotaBytes>>,
	initial_chunk: Bytes,
	max_size: usize,
	timeout_sec: u64,
) {
	let mut buf = [0u8; 8192];
	let mut total_bytes = initial_chunk.len();

	if !initial_chunk.is_empty() {
		match QuotaBytes::new(initial_chunk) {
			Ok(qb) => {
				if tx.send(Ok(qb)).await.is_err() {
					return;
				}
			}
			Err(e) => {
				let _ = tx.send(Err(e)).await;
				return;
			}
		}
	}

	loop {
		let read_future = stdout.read(&mut buf);
		match timeout(Duration::from_secs(timeout_sec), read_future).await {
			Ok(Ok(0)) => {
				log(
					LogLevel::Debug,
					&format!("✓ CGI Body Pump EOF. Total: {total_bytes} bytes"),
				);
				break;
			}
			Ok(Ok(n)) => {
				total_bytes += n;
				if total_bytes > max_size {
					log(LogLevel::Error, "✗ CGI Body Exceeded Max Size.");
					let _ = tx
						.send(Err(Error::System("CGI Body Exceeded Max Size".into())))
						.await;
					return;
				}

				let data = Bytes::copy_from_slice(&buf[..n]);
				match QuotaBytes::new(data) {
					Ok(qb) => {
						if tx.send(Ok(qb)).await.is_err() {
							break;
						}
					}
					Err(e) => {
						let _ = tx.send(Err(e)).await;
						return;
					}
				}
			}
			Ok(Err(e)) => {
				log(LogLevel::Error, &format!("✗ CGI Read Error: {e}"));
				let _ = tx.send(Err(Error::System(e.to_string()))).await;
				break;
			}
			Err(_) => {
				log(LogLevel::Error, "✗ CGI Body Idle Timeout.");
				let _ = tx
					.send(Err(Error::System("CGI Body Idle Timeout".into())))
					.await;
				return;
			}
		}
	}
}
