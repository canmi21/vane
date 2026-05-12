//! [`H3Body`] — `http_body::Body` adapter over the [h3] crate's split
//! `recv_data` / `recv_trailers` stream surface, for both
//! `h3::server::RequestStream` and `h3::client::RequestStream`.
//!
//! `h3` exposes its body shape as two separate calls — one returning
//! `impl Buf` data chunks and a once-only `recv_trailers()` after the
//! data half closes. [`H3Body`] embeds those two calls directly inside
//! an `async-stream`-backed generator that drives `recv_data` until
//! end-of-stream and then `recv_trailers` once; [`http_body::Body::poll_frame`]
//! polls that single self-contained future. No `tokio::spawn`, no
//! `mpsc` channel — the consumer's poll drives h3 directly so a slow
//! consumer naturally throttles h3 reads through the QUIC flow-control
//! window.
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
use futures_core::Stream;
use http::HeaderMap;
use http_body::{Body, Frame};
use pin_project_lite::pin_project;

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

pin_project! {
	/// `http_body::Body` adapter over a trait-erased h3 stream source.
	/// Construct via `H3Body::new(source)` at every H3 ingress site
	/// (server or client).
	///
	/// Internally backed by an `async-stream` generator whose state
	/// machine owns the underlying `H3StreamSource` and drives
	/// `recv_data` until end-of-stream then `recv_trailers` once.
	/// `poll_frame` polls that generator directly — no `tokio::spawn`,
	/// no `mpsc::channel`. Backpressure flows through QUIC's flow-
	/// control window: a slow `poll_frame` consumer stops draining
	/// `recv_data`, which lets the QUIC stack signal the peer to
	/// throttle.
	pub struct H3Body {
		#[pin]
		stream: Pin<Box<dyn Stream<Item = io::Result<Frame<Bytes>>> + Send>>,
	}
}

impl H3Body {
	#[must_use]
	pub fn new<S: H3StreamSource + 'static>(mut source: S) -> Self {
		// `async_stream::try_stream!` lets us write the state machine
		// straight through. Yield-and-await pattern preserves
		// readability while collapsing the prior pump task into a
		// single self-pinning future driven by the consumer.
		let stream = async_stream::stream! {
			loop {
				match source.recv_data().await {
					Ok(Some(b)) => yield Ok(Frame::data(b)),
					Ok(None) => break,
					Err(e) => {
						yield Err(e);
						return;
					}
				}
			}
			match source.recv_trailers().await {
				Ok(Some(t)) => yield Ok(Frame::trailers(t)),
				Ok(None) => {}
				Err(e) => yield Err(e),
			}
		};
		Self { stream: Box::pin(stream) }
	}
}

impl Body for H3Body {
	type Data = Bytes;
	type Error = io::Error;

	fn poll_frame(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
		self.project().stream.poll_next(cx)
	}
}
