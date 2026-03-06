use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};

use tokio::io::{self, AsyncRead, AsyncWrite, ReadBuf};

pub(crate) fn now_millis() -> u64 {
	std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.unwrap_or_default()
		.as_millis() as u64
}

pub struct IdleWatchdog<S> {
	inner: S,
	last_activity: Arc<AtomicU64>,
}

impl<S> IdleWatchdog<S> {
	pub fn new(inner: S, last_activity: Arc<AtomicU64>) -> Self {
		Self {
			inner,
			last_activity,
		}
	}

	fn update_activity(&self) {
		self.last_activity.store(now_millis(), Ordering::Relaxed);
	}
}

impl<S: AsyncRead + Unpin> AsyncRead for IdleWatchdog<S> {
	fn poll_read(
		mut self: Pin<&mut Self>,
		cx: &mut Context<'_>,
		buf: &mut ReadBuf<'_>,
	) -> Poll<io::Result<()>> {
		let before = buf.filled().len();
		let p = Pin::new(&mut self.inner).poll_read(cx, buf);
		if matches!(p, Poll::Ready(Ok(()))) && buf.filled().len() > before {
			self.update_activity();
		}
		p
	}
}

impl<S: AsyncWrite + Unpin> AsyncWrite for IdleWatchdog<S> {
	fn poll_write(
		mut self: Pin<&mut Self>,
		cx: &mut Context<'_>,
		buf: &[u8],
	) -> Poll<io::Result<usize>> {
		let p = Pin::new(&mut self.inner).poll_write(cx, buf);
		if let Poll::Ready(Ok(n)) = p
			&& n > 0
		{
			self.update_activity();
		}
		p
	}

	fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
		Pin::new(&mut self.inner).poll_flush(cx)
	}

	fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
		Pin::new(&mut self.inner).poll_shutdown(cx)
	}
}
