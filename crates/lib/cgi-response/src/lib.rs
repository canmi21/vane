//! Parse a CGI child's stdout into an `http::Response`, then stream
//! the body through `http_body::Body`. The other half of a CGI
//! gateway — building the RFC 3875 environment for the child —
//! lives in the `cgi-request` crate; pair them when you need both
//! directions.
//!
//! See the crate-level README for what is and isn't covered here.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use http::{HeaderName, HeaderValue, StatusCode};
use http_body::{Body, Frame, SizeHint};
use tokio::io::{AsyncRead, AsyncReadExt as _, ReadBuf};
use tokio::time::Instant;

/// Errors from [`read_until_header_end`].
#[derive(Debug, thiserror::Error)]
pub enum HeaderReadError {
	/// The reader closed (`read` returned 0) or errored before the
	/// `\r\n\r\n` separator was seen. Hosts typically map this to
	/// `502 Bad Gateway`.
	#[error("cgi child closed before producing a usable header block")]
	Eof,
	/// The deadline expired before the header block completed. Hosts
	/// typically map this to `504 Gateway Timeout`.
	#[error("cgi connect timeout exceeded before header block ended")]
	Timeout,
}

/// Read from `stdout` until the RFC 3875 header / body separator
/// (`\r\n\r\n`), or until `deadline`. Returns the header block (up
/// to and including the separator), the leftover bytes that
/// arrived in the same `read()` past the separator, and the
/// still-open reader for downstream body streaming.
///
/// # Errors
///
/// - [`HeaderReadError::Eof`] when the reader closes / errors
///   before the separator.
/// - [`HeaderReadError::Timeout`] when `deadline` expires first.
pub async fn read_until_header_end<R>(
	mut stdout: R,
	deadline: Instant,
) -> Result<(Vec<u8>, Vec<u8>, R), HeaderReadError>
where
	R: AsyncRead + Unpin + Send,
{
	let mut buf = Vec::with_capacity(1024);
	let mut tmp = [0u8; 4096];
	loop {
		let read = tokio::time::timeout_at(deadline, stdout.read(&mut tmp))
			.await
			.map_err(|_| HeaderReadError::Timeout)?;
		match read {
			Ok(n) if n > 0 => {
				buf.extend_from_slice(&tmp[..n]);
				if let Some(end) = find_header_end(&buf) {
					let leftover = buf.split_off(end);
					return Ok((buf, leftover, stdout));
				}
			}
			// EOF (n == 0) or read error — both mean the child won't
			// produce any more bytes; map to "no usable header block".
			Ok(_) | Err(_) => return Err(HeaderReadError::Eof),
		}
	}
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
	buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4)
}

/// Errors from [`parse_response_headers`].
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
	#[error("non-utf8 header block: {0}")]
	NonUtf8(String),
	#[error("malformed header line: {0}")]
	MalformedHeader(String),
	#[error("invalid header name {0}")]
	InvalidHeaderName(String),
	#[error("invalid header value for {0}")]
	InvalidHeaderValue(String),
	#[error("invalid Status header: {0}")]
	InvalidStatus(String),
}

/// Build an `http::response::Builder` from an RFC 3875 header
/// block. Status resolution:
///
/// * `Status: 200 OK` → status code (CGI-specific header, not an
///   HTTP/1.1 status line).
/// * `Location: /...` without a `Status:` → 302 Found.
/// * No `Status:`, no `Location:` → 200 OK.
///
/// Other headers pass through untouched.
///
/// # Errors
///
/// As [`ParseError`].
pub fn parse_response_headers(block: &[u8]) -> Result<http::response::Builder, ParseError> {
	let s = std::str::from_utf8(block).map_err(|e| ParseError::NonUtf8(e.to_string()))?;
	let mut status: Option<StatusCode> = None;
	let mut location_seen = false;
	let mut builder = http::Response::builder();
	for line in s.split("\r\n") {
		if line.is_empty() {
			continue;
		}
		let (name, value) =
			line.split_once(':').ok_or_else(|| ParseError::MalformedHeader(line.to_owned()))?;
		let name = name.trim();
		let value = value.trim();
		if name.eq_ignore_ascii_case("Status") {
			let code = value
				.split_whitespace()
				.next()
				.ok_or_else(|| ParseError::InvalidStatus(format!("empty value: {value:?}")))?;
			let parsed: u16 =
				code.parse().map_err(|e| ParseError::InvalidStatus(format!("parse {code:?}: {e}")))?;
			status =
				Some(StatusCode::from_u16(parsed).map_err(|e| ParseError::InvalidStatus(e.to_string()))?);
		} else {
			let header_name =
				HeaderName::try_from(name).map_err(|_| ParseError::InvalidHeaderName(name.to_owned()))?;
			let header_val = HeaderValue::try_from(value)
				.map_err(|_| ParseError::InvalidHeaderValue(name.to_owned()))?;
			if header_name.as_str().eq_ignore_ascii_case("location") {
				location_seen = true;
			}
			builder = builder.header(header_name, header_val);
		}
	}
	let final_status = match (status, location_seen) {
		(Some(s), _) => s,
		(None, true) => StatusCode::FOUND,
		(None, false) => StatusCode::OK,
	};
	Ok(builder.status(final_status))
}

/// Streaming body for a CGI response: yields the leftover bytes
/// (from the post-header read) first, then reads the rest from the
/// child's stdout to EOF. A `total_deadline` caps the total
/// streaming time; mid-body the next `poll_frame` past the deadline
/// returns an `io::Error`.
///
/// `G` is a generic drop guard that the body owns for its
/// lifetime — typically a permit, an `Arc`, or a cancellation
/// guard the host wants to keep alive while bytes are still
/// flowing. Use `()` when you don't need one.
pub struct CgiResponseBody<R, G = ()> {
	initial: Option<Bytes>,
	stdout: R,
	deadline: Instant,
	_guard: G,
}

impl<R, G> CgiResponseBody<R, G> {
	/// Build a body from a leftover-bytes prefix, an open reader,
	/// the wall-clock deadline for stream completion, and a
	/// caller-supplied drop guard.
	pub fn new(initial: Vec<u8>, stdout: R, deadline: Instant, guard: G) -> Self {
		let initial = if initial.is_empty() { None } else { Some(Bytes::from(initial)) };
		Self { initial, stdout, deadline, _guard: guard }
	}
}

impl<R, G> Body for CgiResponseBody<R, G>
where
	R: AsyncRead + Unpin + Send,
	G: Send + Unpin + 'static,
{
	type Data = Bytes;
	type Error = io::Error;

	fn poll_frame(
		mut self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
		if let Some(b) = self.initial.take() {
			return Poll::Ready(Some(Ok(Frame::data(b))));
		}
		if Instant::now() >= self.deadline {
			return Poll::Ready(Some(Err(io::Error::other("cgi total_timeout exceeded mid-body"))));
		}
		let mut buf = [0u8; 4096];
		let mut read_buf = ReadBuf::new(&mut buf);
		match Pin::new(&mut self.stdout).poll_read(cx, &mut read_buf) {
			Poll::Pending => Poll::Pending,
			Poll::Ready(Ok(())) => {
				let filled = read_buf.filled();
				if filled.is_empty() {
					Poll::Ready(None)
				} else {
					Poll::Ready(Some(Ok(Frame::data(Bytes::copy_from_slice(filled)))))
				}
			}
			Poll::Ready(Err(e)) => {
				Poll::Ready(Some(Err(io::Error::other(format!("cgi stdout read: {e}")))))
			}
		}
	}

	fn is_end_stream(&self) -> bool {
		false
	}

	fn size_hint(&self) -> SizeHint {
		SizeHint::default()
	}
}

#[cfg(test)]
mod tests {
	use std::time::Duration;

	use http_body_util::BodyExt as _;
	use tokio::io::AsyncWriteExt as _;

	use super::*;

	#[test]
	fn parse_status_header_picks_up_code() {
		let block = b"Status: 201 Created\r\nContent-Type: text/plain\r\n\r\n";
		let resp = parse_response_headers(block).expect("parse").body(()).unwrap();
		assert_eq!(resp.status(), StatusCode::CREATED);
		assert_eq!(resp.headers().get("content-type").unwrap(), "text/plain");
	}

	#[test]
	fn parse_location_without_status_defaults_to_302() {
		let block = b"Location: /elsewhere\r\n\r\n";
		let resp = parse_response_headers(block).expect("parse").body(()).unwrap();
		assert_eq!(resp.status(), StatusCode::FOUND);
		assert_eq!(resp.headers().get("location").unwrap(), "/elsewhere");
	}

	#[test]
	fn parse_no_status_no_location_defaults_to_200() {
		let block = b"Content-Type: text/plain\r\n\r\n";
		let resp = parse_response_headers(block).expect("parse").body(()).unwrap();
		assert_eq!(resp.status(), StatusCode::OK);
	}

	#[test]
	fn parse_rejects_malformed_line() {
		let block = b"no-colon-here\r\n\r\n";
		assert!(matches!(parse_response_headers(block), Err(ParseError::MalformedHeader(_)),));
	}

	#[tokio::test]
	async fn read_until_header_end_returns_block_and_leftover() {
		let (mut tx, rx) = tokio::io::duplex(64);
		tokio::spawn(async move {
			tx.write_all(b"Status: 200 OK\r\n\r\nbody-bytes-here").await.unwrap();
		});
		let deadline = Instant::now() + Duration::from_secs(2);
		let (head, leftover, _rest) = read_until_header_end(rx, deadline).await.expect("ok");
		assert_eq!(head, b"Status: 200 OK\r\n\r\n");
		assert_eq!(leftover, b"body-bytes-here");
	}

	#[tokio::test]
	async fn read_until_header_end_eof_returns_err() {
		let (tx, rx) = tokio::io::duplex(64);
		drop(tx); // immediate EOF
		let deadline = Instant::now() + Duration::from_secs(2);
		assert!(matches!(read_until_header_end(rx, deadline).await, Err(HeaderReadError::Eof)));
	}

	#[tokio::test(start_paused = true)]
	async fn read_until_header_end_timeout_returns_err() {
		let (_tx, rx) = tokio::io::duplex(64);
		// Hold _tx open without writing so the read just waits.
		let deadline = Instant::now() + Duration::from_millis(50);
		tokio::time::advance(Duration::from_millis(60)).await;
		assert!(matches!(read_until_header_end(rx, deadline).await, Err(HeaderReadError::Timeout)));
	}

	#[tokio::test]
	async fn cgi_response_body_yields_leftover_then_streams_to_eof() {
		let (mut tx, rx) = tokio::io::duplex(64);
		tokio::spawn(async move {
			tx.write_all(b"-streamed").await.unwrap();
			drop(tx);
		});
		let deadline = Instant::now() + Duration::from_secs(2);
		let mut body = CgiResponseBody::new(b"leftover".to_vec(), rx, deadline, ());

		let frame = std::pin::Pin::new(&mut body).frame().await.expect("first frame").expect("data");
		assert_eq!(frame.into_data().unwrap(), &b"leftover"[..]);

		// Next: at least one stdout chunk before EOF.
		let mut acc = Vec::new();
		while let Some(f) = std::pin::Pin::new(&mut body).frame().await {
			acc.extend_from_slice(f.unwrap().into_data().unwrap().as_ref());
		}
		assert_eq!(acc, b"-streamed");
	}

	#[tokio::test]
	async fn cgi_response_body_keeps_guard_alive_until_drop() {
		// Use an Arc<()> as the drop guard; we hold a weak ref and
		// confirm strong-count > 0 while the body lives, drops to 0
		// after.
		let guard = std::sync::Arc::new(());
		let weak = std::sync::Arc::downgrade(&guard);
		let (_tx, rx) = tokio::io::duplex(64);
		let body = CgiResponseBody::new(Vec::new(), rx, Instant::now() + Duration::from_secs(1), guard);
		assert!(weak.strong_count() > 0);
		drop(body);
		assert_eq!(weak.strong_count(), 0);
	}
}
