//! [`H3Body`] — `http_body::Body` adapter over the [h3] crate's split
//! `recv_data` / `recv_trailers` stream surface, for both
//! `h3::server::RequestStream` and `h3::client::RequestStream`.
//!
//! `h3` exposes its body shape as two separate calls — one returning
//! `impl Buf` data chunks and a once-only `recv_trailers()` after the
//! data half closes. [`H3Body`] runs a small pump task that walks both
//! calls in order and pushes each result onto a bounded channel;
//! [`http_body::Body::poll_frame`] simply forwards the channel.
//!
//! [`ServerStreamSource`] adapts the listener-side stream — used to
//! feed inbound request bodies into an HTTP stack. [`ClientStreamSource`]
//! adapts the upstream-side stream — used to surface upstream response
//! bodies as `http_body::Body` for downstream pipelines.
//!
//! [h3]: https://crates.io/crates/h3

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use async_trait::async_trait;
use bytes::{Buf, Bytes};
use http::HeaderMap;
use http_body::{Body, Frame};
use tokio::sync::mpsc;

/// Backpressure cap for the body channel — chosen to balance burstiness
/// (h3's `recv_data` may produce small frames quickly) against the cost
/// of a stalled consumer holding a memory ceiling. The channel is
/// per-stream so the cap is per-stream too.
const FRAME_CHANNEL_CAPACITY: usize = 8;

/// Trait that hides the difference between `h3::server::RequestStream`
/// and `h3::client::RequestStream`. Each impl adapts h3's two-call
/// surface (`recv_data` then `recv_trailers`) into discrete async
/// helpers that [`H3Body`]'s pump task orchestrates.
#[async_trait]
pub trait H3StreamSource: Send {
	/// Pull the next data chunk. `Ok(Some(_))` is more bytes;
	/// `Ok(None)` signals the data half is closed.
	async fn recv_data(&mut self) -> io::Result<Option<Bytes>>;

	/// Pull the optional trailers block. Called exactly once after
	/// `recv_data` returns `Ok(None)`. `Ok(None)` means no trailers.
	async fn recv_trailers(&mut self) -> io::Result<Option<HeaderMap>>;
}

/// Server-side h3 stream source. Wraps `h3::server::RequestStream`
/// and adapts the buffer chain that `recv_data` returns (an `impl
/// Buf` whose remaining slice is a contiguous `Bytes`) into a single
/// `Bytes` chunk per call.
///
/// The bound is `h3::quic::RecvStream` (not `BidiStream`) so this
/// adapter accepts the recv half of a `RequestStream::split` —
/// allowing the request body to stream concurrently with response
/// writeback on the send half.
pub struct ServerStreamSource<S: h3::quic::RecvStream> {
	inner: h3::server::RequestStream<S, Bytes>,
	trailers_done: bool,
}

impl<S> ServerStreamSource<S>
where
	S: h3::quic::RecvStream + Send,
{
	pub fn new(inner: h3::server::RequestStream<S, Bytes>) -> Self {
		Self { inner, trailers_done: false }
	}
}

#[async_trait]
impl<S> H3StreamSource for ServerStreamSource<S>
where
	S: h3::quic::RecvStream + Send,
{
	async fn recv_data(&mut self) -> io::Result<Option<Bytes>> {
		match self.inner.recv_data().await {
			Ok(Some(mut buf)) => {
				let remaining = buf.remaining();
				let bytes = buf.copy_to_bytes(remaining);
				Ok(Some(bytes))
			}
			Ok(None) => Ok(None),
			Err(e) => Err(io::Error::other(format!("h3 recv_data: {e}"))),
		}
	}

	async fn recv_trailers(&mut self) -> io::Result<Option<HeaderMap>> {
		if self.trailers_done {
			return Ok(None);
		}
		self.trailers_done = true;
		match self.inner.recv_trailers().await {
			Ok(opt) => Ok(opt),
			Err(e) => Err(io::Error::other(format!("h3 recv_trailers: {e}"))),
		}
	}
}

/// Client-side h3 stream source. Wraps `h3::client::RequestStream` and
/// adapts the same two-call surface as the server-side adapter
/// (`recv_data` then `recv_trailers`) so [`H3Body`] can drive both
/// ends uniformly. Used to wrap an upstream's response stream as an
/// `http_body::Body`.
///
/// The bound is `h3::quic::RecvStream` (not `BidiStream`) so this
/// adapter accepts both the bidi stream returned by
/// `SendRequest::send_request` (used in-place for the request /
/// response round-trip) and the recv half of a future
/// `RequestStream::split` if the upstream path needs concurrent
/// request-body upload and response-body read.
pub struct ClientStreamSource<S: h3::quic::RecvStream> {
	inner: h3::client::RequestStream<S, Bytes>,
	trailers_done: bool,
}

impl<S> ClientStreamSource<S>
where
	S: h3::quic::RecvStream + Send,
{
	pub fn new(inner: h3::client::RequestStream<S, Bytes>) -> Self {
		Self { inner, trailers_done: false }
	}
}

#[async_trait]
impl<S> H3StreamSource for ClientStreamSource<S>
where
	S: h3::quic::RecvStream + Send,
{
	async fn recv_data(&mut self) -> io::Result<Option<Bytes>> {
		match self.inner.recv_data().await {
			Ok(Some(mut buf)) => {
				let remaining = buf.remaining();
				let bytes = buf.copy_to_bytes(remaining);
				Ok(Some(bytes))
			}
			Ok(None) => Ok(None),
			Err(e) => Err(io::Error::other(format!("h3 client recv_data: {e}"))),
		}
	}

	async fn recv_trailers(&mut self) -> io::Result<Option<HeaderMap>> {
		if self.trailers_done {
			return Ok(None);
		}
		self.trailers_done = true;
		match self.inner.recv_trailers().await {
			Ok(opt) => Ok(opt),
			Err(e) => Err(io::Error::other(format!("h3 client recv_trailers: {e}"))),
		}
	}
}

/// `http_body::Body` adapter over a trait-erased h3 stream source.
/// Construct via `H3Body::new(source)` at every H3 ingress site
/// (server or client).
///
/// A spawned pump task drives `recv_data` then `recv_trailers` and
/// pushes each result onto a bounded channel. `poll_frame` is a thin
/// wrapper around `Receiver::poll_recv`. The pump exits cleanly when
/// the consumer drops `H3Body` — `tx.send` returns `Err`, the loop
/// breaks, and the source's drop frees the underlying h3 stream.
pub struct H3Body {
	rx: mpsc::Receiver<io::Result<Frame<Bytes>>>,
}

impl H3Body {
	#[must_use]
	pub fn new<S: H3StreamSource + 'static>(mut source: S) -> Self {
		let (tx, rx) = mpsc::channel(FRAME_CHANNEL_CAPACITY);
		tokio::spawn(async move {
			loop {
				match source.recv_data().await {
					Ok(Some(b)) => {
						if tx.send(Ok(Frame::data(b))).await.is_err() {
							return;
						}
					}
					Ok(None) => break,
					Err(e) => {
						let _ = tx.send(Err(e)).await;
						return;
					}
				}
			}
			match source.recv_trailers().await {
				Ok(Some(t)) => {
					let _ = tx.send(Ok(Frame::trailers(t))).await;
				}
				Ok(None) => {}
				Err(e) => {
					let _ = tx.send(Err(e)).await;
				}
			}
		});
		Self { rx }
	}
}

impl Body for H3Body {
	type Data = Bytes;
	type Error = io::Error;

	fn poll_frame(
		mut self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
		self.rx.poll_recv(cx)
	}
}
