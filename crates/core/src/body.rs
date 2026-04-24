use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use http_body::{Body as HttpBody, Frame, SizeHint};

use crate::error::Error;

pub type Request = http::Request<Body>;
pub type Response = http::Response<Body>;

pub enum Body {
	Static(Bytes),
	Empty,
	Stream(Pin<Box<dyn HttpBody<Data = Bytes, Error = Error> + Send + 'static>>),
}

impl Body {
	#[must_use]
	pub fn as_static(&self) -> Option<&Bytes> {
		if let Self::Static(b) = self { Some(b) } else { None }
	}

	pub fn from_producer<B, E>(producer: B) -> Self
	where
		B: HttpBody<Data = Bytes, Error = E> + Send + 'static,
		E: Into<Error> + Send + Sync + 'static,
	{
		Self::Stream(Box::pin(BodyStreamAdapter { inner: Box::pin(producer) }))
	}
}

impl HttpBody for Body {
	type Data = Bytes;
	type Error = Error;

	fn poll_frame(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
		match self.get_mut() {
			Self::Static(b) => {
				if b.is_empty() {
					Poll::Ready(None)
				} else {
					let out = std::mem::take(b);
					Poll::Ready(Some(Ok(Frame::data(out))))
				}
			}
			Self::Empty => Poll::Ready(None),
			Self::Stream(s) => s.as_mut().poll_frame(cx),
		}
	}

	fn is_end_stream(&self) -> bool {
		match self {
			Self::Static(b) => b.is_empty(),
			Self::Empty => true,
			Self::Stream(s) => s.is_end_stream(),
		}
	}

	fn size_hint(&self) -> SizeHint {
		match self {
			Self::Static(b) => SizeHint::with_exact(b.len() as u64),
			Self::Empty => SizeHint::with_exact(0),
			Self::Stream(s) => s.size_hint(),
		}
	}
}

// `inner` is `Pin<Box<B>>` rather than `B` so we can poll without unsafe pin
// projection; the extra heap indirection is the price of `unsafe_code = deny`.
pub struct BodyStreamAdapter<B> {
	inner: Pin<Box<B>>,
}

impl<B, E> HttpBody for BodyStreamAdapter<B>
where
	B: HttpBody<Data = Bytes, Error = E> + Send + 'static,
	E: Into<Error> + Send + Sync + 'static,
{
	type Data = Bytes;
	type Error = Error;

	fn poll_frame(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
		match self.get_mut().inner.as_mut().poll_frame(cx) {
			Poll::Pending => Poll::Pending,
			Poll::Ready(None) => Poll::Ready(None),
			Poll::Ready(Some(Ok(f))) => Poll::Ready(Some(Ok(f))),
			Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(e.into()))),
		}
	}

	fn is_end_stream(&self) -> bool {
		self.inner.is_end_stream()
	}

	fn size_hint(&self) -> SizeHint {
		self.inner.size_hint()
	}
}
