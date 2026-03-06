use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};

use tokio::io::{self, AsyncRead, AsyncWrite, ReadBuf};

pub fn now_millis() -> u64 {
	std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis()
		as u64
}

pub struct IdleWatchdog<S> {
	inner: S,
	last_activity: Arc<AtomicU64>,
}

impl<S> IdleWatchdog<S> {
	pub const fn new(inner: S, last_activity: Arc<AtomicU64>) -> Self {
		Self { inner, last_activity }
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
	use super::*;
	use std::sync::atomic::Ordering;
	use tokio::io::{AsyncReadExt, AsyncWriteExt};

	#[test]
	fn now_millis_returns_nonzero() {
		assert!(now_millis() > 0);
	}

	#[tokio::test]
	async fn watchdog_updates_activity_on_read() {
		let last_activity = Arc::new(AtomicU64::new(0));

		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let addr = listener.local_addr().unwrap();

		let write_task = tokio::spawn(async move {
			let mut conn = tokio::net::TcpStream::connect(addr).await.unwrap();
			conn.write_all(b"hello").await.unwrap();
			conn
		});

		let (server, _) = listener.accept().await.unwrap();
		let _client = write_task.await.unwrap();

		let mut watchdog = IdleWatchdog::new(server, last_activity.clone());
		assert_eq!(last_activity.load(Ordering::Relaxed), 0);

		let mut buf = [0u8; 5];
		watchdog.read_exact(&mut buf).await.unwrap();
		assert_eq!(&buf, b"hello");

		let activity = last_activity.load(Ordering::Relaxed);
		assert!(activity > 0, "last_activity should have been updated");
	}
}
