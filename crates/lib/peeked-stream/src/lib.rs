//! `PeekedStream<S>`: an [`AsyncRead`] / [`AsyncWrite`] adapter that
//! prepends a previously-buffered byte sequence to the read side of
//! `S` while passing writes through unchanged.
//!
//! A common pattern in protocol-detecting servers is to peek the first
//! bytes of a freshly accepted connection, decide which decoder to
//! engage (TLS / HTTP/1 / HTTP/2 preface / opaque L4), then hand the
//! stream to that decoder. Whichever consumer wakes up next must
//! observe the peeked bytes from offset zero — as though no read had
//! happened. Wrapping the stream in `PeekedStream { buffer: peeked,
//! inner: stream }` rewinds the buffer into the read path: `poll_read`
//! drains `buffer` first, then delegates to `inner`. Writes / flushes
//! / shutdowns pass through to the inner stream untouched.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

pub struct PeekedStream<S> {
	buffer: Bytes,
	inner: S,
}

impl<S> PeekedStream<S> {
	#[must_use]
	pub const fn new(buffer: Bytes, inner: S) -> Self {
		Self { buffer, inner }
	}

	/// Drop the peek buffer (regardless of whether it was drained) and
	/// return both pieces. Useful when the caller needs the concrete
	/// inner type (e.g. a `TcpStream` for `set_nodelay` / `peer_addr`)
	/// after the peek phase has resolved.
	pub fn into_inner(self) -> (Bytes, S) {
		(self.buffer, self.inner)
	}

	/// Borrow the inner stream — useful when callers only need to
	/// invoke socket-level methods that don't touch the read cursor.
	pub const fn inner_ref(&self) -> &S {
		&self.inner
	}
}

impl<S: AsyncRead + Unpin> AsyncRead for PeekedStream<S> {
	fn poll_read(
		mut self: Pin<&mut Self>,
		cx: &mut Context<'_>,
		buf: &mut ReadBuf<'_>,
	) -> Poll<io::Result<()>> {
		if !self.buffer.is_empty() {
			let take = self.buffer.len().min(buf.remaining());
			let head = self.buffer.split_to(take);
			buf.put_slice(&head);
			return Poll::Ready(Ok(()));
		}
		Pin::new(&mut self.inner).poll_read(cx, buf)
	}
}

impl<S: AsyncWrite + Unpin> AsyncWrite for PeekedStream<S> {
	fn poll_write(
		mut self: Pin<&mut Self>,
		cx: &mut Context<'_>,
		buf: &[u8],
	) -> Poll<io::Result<usize>> {
		Pin::new(&mut self.inner).poll_write(cx, buf)
	}

	fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
		Pin::new(&mut self.inner).poll_flush(cx)
	}

	fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
		Pin::new(&mut self.inner).poll_shutdown(cx)
	}
}

#[cfg(test)]
mod tests {
	use bytes::Bytes;
	use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};

	use super::PeekedStream;

	#[tokio::test]
	async fn read_drains_buffer_before_inner() {
		let (mut peer, inner) = duplex(64);
		peer.write_all(b"INNER").await.expect("write inner");
		drop(peer);
		let mut s = PeekedStream::new(Bytes::from_static(b"PEEK"), inner);
		let mut out = Vec::new();
		s.read_to_end(&mut out).await.expect("read_to_end");
		assert_eq!(out, b"PEEKINNER");
	}

	#[tokio::test]
	async fn read_handles_partial_consumer_buffer_across_boundary() {
		let (mut peer, inner) = duplex(64);
		peer.write_all(b"INNER").await.expect("write inner");
		drop(peer);
		let mut s = PeekedStream::new(Bytes::from_static(b"PEEKED"), inner);

		let mut head = [0u8; 3];
		s.read_exact(&mut head).await.expect("read head");
		assert_eq!(&head, b"PEE");
		let mut tail = [0u8; 3];
		s.read_exact(&mut tail).await.expect("read tail");
		assert_eq!(&tail, b"KED");
		let mut rest = Vec::new();
		s.read_to_end(&mut rest).await.expect("read rest");
		assert_eq!(rest, b"INNER");
	}

	#[tokio::test]
	async fn read_with_empty_buffer_passes_through_to_inner() {
		let (mut peer, inner) = duplex(64);
		peer.write_all(b"DATA").await.expect("write inner");
		drop(peer);
		let mut s = PeekedStream::new(Bytes::new(), inner);
		let mut out = Vec::new();
		s.read_to_end(&mut out).await.expect("read");
		assert_eq!(out, b"DATA");
	}

	#[tokio::test]
	async fn write_passes_through_to_inner() {
		let (peer, inner) = duplex(64);
		let mut s = PeekedStream::new(Bytes::from_static(b"PEEK"), inner);
		s.write_all(b"OUT").await.expect("write");
		s.flush().await.expect("flush");
		drop(s);
		let mut peer = peer;
		let mut got = Vec::new();
		peer.read_to_end(&mut got).await.expect("peer read");
		assert_eq!(got, b"OUT");
	}

	#[tokio::test]
	async fn shutdown_passes_through_to_inner() {
		let (peer, inner) = duplex(64);
		let mut s = PeekedStream::new(Bytes::from_static(b"PEEK"), inner);
		s.shutdown().await.expect("shutdown");
		let mut peer = peer;
		let mut got = Vec::new();
		peer.read_to_end(&mut got).await.expect("peer read post-shutdown");
		assert!(got.is_empty(), "peer saw unexpected bytes: {got:?}");
	}

	#[tokio::test]
	async fn into_inner_returns_remaining_buffer_and_inner() {
		let (_peer, inner) = duplex(64);
		let buffer = Bytes::from_static(b"PEEK");
		let mut s = PeekedStream::new(buffer.clone(), inner);
		let mut head = [0u8; 2];
		s.read_exact(&mut head).await.expect("read head");
		let (residual, _inner) = s.into_inner();
		assert_eq!(&*residual, b"EK");
	}
}
