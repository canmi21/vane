use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use http_body::{Body as HttpBody, Frame, SizeHint};
use pin_project_lite::pin_project;

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
		// One heap allocation — for the outer `dyn HttpBody` —
		// instead of two. The inner adapter pin-projects to its
		// stored producer in place via `pin_project_lite`, so we
		// drop the prior `inner: Pin<Box<B>>` layer that existed
		// only to dodge an `unsafe` projection.
		Self::Stream(Box::pin(BodyStreamAdapter { inner: producer }))
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

pin_project! {
	/// Erases the producer's error type into [`Error`] via the
	/// caller-supplied `Into` impl, leaving the rest of the
	/// [`HttpBody`] surface untouched. `pin_project_lite` generates a
	/// safe `project()` so the inner producer is stored by value and
	/// projected without an `unsafe` block (the prior shape stored it
	/// behind a `Pin<Box<B>>` purely to dodge `unsafe`).
	pub struct BodyStreamAdapter<B> {
		#[pin]
		inner: B,
	}
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
		match self.project().inner.poll_frame(cx) {
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

#[cfg(test)]
mod tests {
	use std::collections::VecDeque;
	use std::task::Waker;

	use super::*;
	use crate::error::{Error, ErrorKind};

	/// A hand-rolled `http_body::Body` fixture driven from a scripted frame queue.
	///
	/// Each `Step` resolves synchronously under one `poll_frame` call; the
	/// producer is constructed with a finite script and panics if polled past
	/// end-of-stream. `E` is the producer's declared error type so tests can
	/// exercise the `E: Into<Error>` conversion path in `BodyStreamAdapter`.
	enum Step<E> {
		Data(Bytes),
		Err(E),
		End,
	}

	type PollFrame<D, E> = Poll<Option<Result<Frame<D>, E>>>;

	struct ScriptedBody<E> {
		steps: VecDeque<Step<E>>,
		size_hint: SizeHint,
		is_end_stream: bool,
	}

	impl<E> ScriptedBody<E> {
		fn new(steps: Vec<Step<E>>) -> Self {
			Self { steps: steps.into(), size_hint: SizeHint::new(), is_end_stream: false }
		}

		fn with_size_hint(mut self, hint: SizeHint) -> Self {
			self.size_hint = hint;
			self
		}

		fn with_end_stream(mut self, end: bool) -> Self {
			self.is_end_stream = end;
			self
		}
	}

	impl<E> HttpBody for ScriptedBody<E>
	where
		E: Unpin,
	{
		type Data = Bytes;
		type Error = E;

		fn poll_frame(
			self: Pin<&mut Self>,
			_cx: &mut Context<'_>,
		) -> PollFrame<Self::Data, Self::Error> {
			let this = self.get_mut();
			match this.steps.pop_front() {
				Some(Step::Data(b)) => Poll::Ready(Some(Ok(Frame::data(b)))),
				Some(Step::Err(e)) => Poll::Ready(Some(Err(e))),
				Some(Step::End) | None => Poll::Ready(None),
			}
		}

		fn is_end_stream(&self) -> bool {
			self.is_end_stream
		}

		fn size_hint(&self) -> SizeHint {
			self.size_hint.clone()
		}
	}

	fn poll_once<B: HttpBody + Unpin>(body: &mut B) -> PollFrame<B::Data, B::Error> {
		let waker = Waker::noop();
		let mut cx = Context::from_waker(waker);
		Pin::new(body).poll_frame(&mut cx)
	}

	#[test]
	fn as_static_returns_inner_bytes_for_static_variant() {
		let payload = Bytes::from_static(b"hello");
		let body = Body::Static(payload.clone());
		let got = body.as_static().expect("static variant must yield Some");
		assert_eq!(got, &payload);
		assert_eq!(got.as_ref(), b"hello");
	}

	#[test]
	fn as_static_returns_none_for_empty_variant() {
		let body = Body::Empty;
		assert!(body.as_static().is_none());
	}

	#[test]
	fn as_static_returns_none_for_stream_variant() {
		let producer: ScriptedBody<Error> = ScriptedBody::new(vec![Step::End]);
		let body = Body::from_producer(producer);
		assert!(body.as_static().is_none());
	}

	#[test]
	fn empty_body_is_end_stream_and_zero_size_hint() {
		let body = Body::Empty;
		assert!(body.is_end_stream());
		let hint = body.size_hint();
		assert_eq!(hint.exact(), Some(0));
	}

	#[test]
	fn static_body_reports_exact_size_and_not_end_of_stream() {
		let body = Body::Static(Bytes::from_static(b"hi"));
		assert!(!body.is_end_stream());
		assert_eq!(body.size_hint().exact(), Some(2));
	}

	#[test]
	fn static_body_yields_payload_then_end_of_stream() {
		let mut body = Body::Static(Bytes::from_static(b"hi"));
		match poll_once(&mut body) {
			Poll::Ready(Some(Ok(frame))) => {
				let data = frame.into_data().expect("first frame must be data");
				assert_eq!(data.as_ref(), b"hi");
			}
			other => panic!("expected ready-data frame, got {other:?}"),
		}
		match poll_once(&mut body) {
			Poll::Ready(None) => {}
			other => panic!("expected end-of-stream after one data frame, got {other:?}"),
		}
	}

	#[test]
	fn empty_body_yields_no_frames() {
		let mut body = Body::Empty;
		match poll_once(&mut body) {
			Poll::Ready(None) => {}
			other => panic!("Body::Empty must immediately report end-of-stream, got {other:?}"),
		}
	}

	#[test]
	fn stream_body_delegates_is_end_stream_and_size_hint_to_inner() {
		let hint = SizeHint::with_exact(42);
		let producer: ScriptedBody<Error> =
			ScriptedBody::new(vec![Step::End]).with_size_hint(hint).with_end_stream(true);
		let body = Body::from_producer(producer);
		assert!(body.is_end_stream(), "Stream variant must forward inner is_end_stream");
		assert_eq!(body.size_hint().exact(), Some(42));
	}

	#[test]
	fn from_producer_passes_data_frames_through_unchanged() {
		let producer: ScriptedBody<Error> = ScriptedBody::new(vec![
			Step::Data(Bytes::from_static(b"one")),
			Step::Data(Bytes::from_static(b"two")),
			Step::End,
		]);
		let mut body = Body::from_producer(producer);

		let Poll::Ready(Some(Ok(f1))) = poll_once(&mut body) else {
			panic!("first poll must yield a data frame");
		};
		assert_eq!(f1.into_data().expect("data frame").as_ref(), b"one");

		let Poll::Ready(Some(Ok(f2))) = poll_once(&mut body) else {
			panic!("second poll must yield a data frame");
		};
		assert_eq!(f2.into_data().expect("data frame").as_ref(), b"two");

		match poll_once(&mut body) {
			Poll::Ready(None) => {}
			other => panic!("exhausted stream must be Ready(None), got {other:?}"),
		}
	}

	#[test]
	fn from_producer_converts_inner_error_via_into() {
		let io_err = std::io::Error::other("scripted-io-failure");
		let producer: ScriptedBody<std::io::Error> = ScriptedBody::new(vec![Step::Err(io_err)]);
		let mut body = Body::from_producer(producer);
		match poll_once(&mut body) {
			Poll::Ready(Some(Err(e))) => {
				assert!(matches!(e.kind(), ErrorKind::Io), "io::Error must map to ErrorKind::Io");
			}
			other => panic!("expected Ready(Some(Err(..))) from failing producer, got {other:?}"),
		}
	}

	#[test]
	fn from_producer_preserves_end_of_stream_signal() {
		let producer: ScriptedBody<Error> = ScriptedBody::new(vec![]);
		let mut body = Body::from_producer(producer);
		match poll_once(&mut body) {
			Poll::Ready(None) => {}
			other => panic!("empty scripted producer must report end-of-stream, got {other:?}"),
		}
	}

	#[test]
	fn from_producer_accepts_serde_json_error_source() {
		let parse_err: serde_json::Error =
			serde_json::from_str::<serde_json::Value>("{not json").unwrap_err();
		let producer: ScriptedBody<serde_json::Error> = ScriptedBody::new(vec![Step::Err(parse_err)]);
		let mut body = Body::from_producer(producer);
		match poll_once(&mut body) {
			Poll::Ready(Some(Err(e))) => {
				assert!(
					matches!(e.kind(), ErrorKind::Compile),
					"serde_json::Error must map to ErrorKind::Compile",
				);
			}
			other => panic!("expected converted Compile error, got {other:?}"),
		}
	}
}
