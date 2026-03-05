/* src/layers/l4/proxy/mod.rs */

pub mod stream;
pub mod tcp;
pub mod udp;

pub use stream::proxy_generic_stream;
pub use tcp::proxy_tcp_stream;
pub use udp::proxy_udp_direct;

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context as TaskContext, Poll};
use tokio::io::{self, AsyncRead, AsyncWrite, ReadBuf};

// --- Idle Watchdog Wrapper ---

pub struct IdleWatchdog<S> {
	inner: S,
	last_activity: Arc<AtomicU64>,
}

impl<S> IdleWatchdog<S> {
	pub fn new(inner: S, last_activity: Arc<AtomicU64>) -> Self {
		Self { inner, last_activity }
	}

	fn update_activity(&self) {
		let now = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap_or_default()
			.as_secs();
		self.last_activity.store(now, Ordering::Relaxed);
	}
}

impl<S: AsyncRead + Unpin> AsyncRead for IdleWatchdog<S> {
	fn poll_read(
		mut self: Pin<&mut Self>,
		cx: &mut TaskContext<'_>,
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
		cx: &mut TaskContext<'_>,
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

	fn poll_flush(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
		Pin::new(&mut self.inner).poll_flush(cx)
	}

	fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
		Pin::new(&mut self.inner).poll_shutdown(cx)
	}
}
