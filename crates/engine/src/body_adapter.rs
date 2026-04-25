//! Adapters from foreign body types to `vane_core::Body`'s required
//! `HttpBody<Data = Bytes, Error = vane_core::Error>` shape.
//!
//! Both server-side decoded request bodies (returned by hyper to the H1
//! service-fn at `Node::Upgrade`) and client-side decoded response bodies
//! (returned by `hyper_util::client::legacy::Client::request` inside
//! `HttpProxyFetch`) arrive as the same `hyper::body::Incoming` type in
//! hyper 1.x. `IncomingAdapter` wraps either side without re-routing
//! through `Body::from_producer`, since we want a clear `HttpBody` layer
//! that produces our own `Error` rather than a `Box<dyn Error>` source.

use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use http_body::{Body as HttpBody, Frame, SizeHint};
use hyper::body::Incoming;
use pin_project_lite::pin_project;
use vane_core::Error;

pin_project! {
	/// Adapts `hyper::body::Incoming` into the `HttpBody<Data = Bytes,
	/// Error = vane_core::Error>` shape required by `vane_core::Body::Stream`.
	/// `pin_project_lite` generates a safe `project()` so we project to
	/// `inner` without an `unsafe` block (CLAUDE.md `unsafe_code = "deny"`).
	pub(crate) struct IncomingAdapter {
		#[pin]
		inner: Incoming,
	}
}

impl IncomingAdapter {
	pub(crate) fn new(inner: Incoming) -> Self {
		Self { inner }
	}
}

impl HttpBody for IncomingAdapter {
	type Data = Bytes;
	type Error = Error;

	fn poll_frame(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
		match self.project().inner.poll_frame(cx) {
			Poll::Pending => Poll::Pending,
			Poll::Ready(None) => Poll::Ready(None),
			Poll::Ready(Some(Ok(f))) => Poll::Ready(Some(Ok(f))),
			Poll::Ready(Some(Err(e))) => {
				Poll::Ready(Some(Err(Error::protocol("hyper incoming body").with_source(e))))
			}
		}
	}

	fn is_end_stream(&self) -> bool {
		self.inner.is_end_stream()
	}

	fn size_hint(&self) -> SizeHint {
		self.inner.size_hint()
	}
}
