//! `H3Body` — engine-side adapter that unifies `h3::server::RequestStream`
//! and (post-S3-02) `h3::client::RequestStream` under a single
//! `http_body::Body` surface. See `spec/architecture/07-l7.md` §
//! `H3Body` (engine-owned).
//!
//! `h3` splits the stream surface across `recv_data() -> impl Buf` and a
//! once-only `recv_trailers()` call at data EOF. `H3Body` runs a small
//! pump task that walks both calls in order, sending each result onto a
//! bounded channel; `poll_frame` simply forwards the channel.
//!
//! Only the server impl ships with this PR. The client `H3StreamSource`
//! impl lands with the H3 upstream track.
//
// TODO(s3-02): add `impl<C: h3::quic::Connection<Bytes>>
// H3StreamSource for h3::client::RequestStream<C::OpenStreams, Bytes>`
// when the H3 upstream / QuicPool work begins.

use std::pin::Pin;
use std::task::{Context, Poll};

use async_trait::async_trait;
use bytes::{Buf, Bytes};
use http::HeaderMap;
use http_body::{Body, Frame};
use tokio::sync::mpsc;
use vane_core::Error;

/// Backpressure cap for the body channel — chosen to balance burstiness
/// (h3's `recv_data` may produce small frames quickly) against the cost
/// of a stalled consumer holding a memory ceiling. The channel is
/// per-stream so the cap is per-stream too.
const FRAME_CHANNEL_CAPACITY: usize = 8;

/// Trait that hides the difference between `h3::server::RequestStream`
/// and `h3::client::RequestStream`. Each impl adapts h3's two-call
/// surface (`recv_data` then `recv_trailers`) into discrete async
/// helpers `H3Body`'s pump task orchestrates.
#[async_trait]
pub trait H3StreamSource: Send {
	/// Pull the next data chunk. `Ok(Some(_))` is more bytes;
	/// `Ok(None)` signals the data half is closed.
	async fn recv_data(&mut self) -> Result<Option<Bytes>, Error>;

	/// Pull the optional trailers block. Called exactly once after
	/// `recv_data` returns `Ok(None)`. `Ok(None)` means no trailers.
	async fn recv_trailers(&mut self) -> Result<Option<HeaderMap>, Error>;
}

/// Server-side h3 stream source. Wraps `h3::server::RequestStream`
/// and adapts the buffer chain that `recv_data` returns (an `impl
/// Buf` whose remaining slice is a contiguous `Bytes`) into a single
/// `Bytes` chunk per call.
///
/// The bound is `h3::quic::RecvStream` (not `BidiStream`) so this
/// adapter accepts the recv half of a `RequestStream::split` —
/// `handle_h3_request` splits the bidi stream so the request body can
/// stream into the executor concurrently with response writeback on
/// the send half.
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
	async fn recv_data(&mut self) -> Result<Option<Bytes>, Error> {
		match self.inner.recv_data().await {
			Ok(Some(mut buf)) => {
				let remaining = buf.remaining();
				let bytes = buf.copy_to_bytes(remaining);
				Ok(Some(bytes))
			}
			Ok(None) => Ok(None),
			Err(e) => {
				Err(Error::protocol("h3 recv_data").with_source(std::io::Error::other(e.to_string())))
			}
		}
	}

	async fn recv_trailers(&mut self) -> Result<Option<HeaderMap>, Error> {
		if self.trailers_done {
			return Ok(None);
		}
		self.trailers_done = true;
		match self.inner.recv_trailers().await {
			Ok(opt) => Ok(opt),
			Err(e) => {
				Err(Error::protocol("h3 recv_trailers").with_source(std::io::Error::other(e.to_string())))
			}
		}
	}
}

/// `http_body::Body` adapter over a trait-erased h3 stream source.
/// Engine constructs `Body::Stream(Box::pin(H3Body::new(source)))` at
/// every H3 ingress site (server or future client).
///
/// A spawned pump task drives `recv_data` then `recv_trailers` and
/// pushes each result onto a bounded channel. `poll_frame` is a thin
/// wrapper around `Receiver::poll_recv`. The pump exits cleanly when
/// the consumer drops `H3Body` — `tx.send` returns `Err`, the loop
/// breaks, and the source's drop frees the underlying h3 stream.
pub struct H3Body {
	rx: mpsc::Receiver<Result<Frame<Bytes>, Error>>,
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
	type Error = Error;

	fn poll_frame(
		mut self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
		self.rx.poll_recv(cx)
	}
}
