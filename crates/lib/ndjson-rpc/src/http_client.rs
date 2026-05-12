//! HTTP-over-TCP management client. Mirrors [`crate::UnixMgmtClient`]
//! but talks to [`crate::http_server`] over `hyper::client::conn::http1`.
//!
//! One TCP connection per call: the management API is verb-at-a-time
//! and not chatty enough to amortize a connection pool. Each call opens
//! a fresh TCP stream, runs the H1 handshake, sends a single POST,
//! consumes either a one-shot JSON body or a chunked NDJSON stream,
//! then drops the connection.

use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::header::{AUTHORIZATION, CONTENT_TYPE, HOST};
use hyper::{Method, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;

use crate::client::{CONNECT_TIMEOUT, MgmtClientError, ONESHOT_TIMEOUT};
use crate::protocol::{Request, Response, ResponseOutcome, WireError, WireErrorKind};

/// Plaintext HTTP/1.1 mgmt client. Cheap to clone — `addr` is `Copy`,
/// `token` is reference-counted.
#[derive(Clone, Debug)]
pub struct HttpMgmtClient {
	addr: SocketAddr,
	token: Option<Arc<str>>,
}

impl HttpMgmtClient {
	/// Build a client targeting the given HTTP endpoint. The `token`
	/// argument matches the server's `bearer_token` setting: pass
	/// `None` only when the server is configured for anonymous access.
	#[must_use]
	pub fn new(addr: SocketAddr, token: Option<Arc<str>>) -> Self {
		Self { addr, token }
	}

	/// One-shot verb call. Mirrors [`crate::UnixMgmtClient::call`].
	///
	/// # Errors
	/// I/O failure ([`MgmtClientError::Io`]); a non-200 HTTP response
	/// ([`MgmtClientError::Http`] — `401` for missing / wrong token,
	/// `400` / `404` / `405` / `413` for malformed requests); a
	/// structured server-side error frame ([`MgmtClientError::Server`]);
	/// or a JSON shape mismatch ([`MgmtClientError::Decode`]).
	pub async fn call<A, R>(&self, verb: &str, args: &A) -> Result<R, MgmtClientError>
	where
		A: serde::Serialize,
		R: for<'de> serde::Deserialize<'de>,
	{
		let req = Request {
			id: 1,
			verb: verb.to_string(),
			args: serde_json::to_value(args).map_err(MgmtClientError::Encode)?,
		};
		let body_bytes = Bytes::from(serde_json::to_vec(&req).map_err(MgmtClientError::Encode)?);
		let resp = self.send(body_bytes).await?;
		let status = resp.status();
		let resp_body = tokio::time::timeout(ONESHOT_TIMEOUT, resp.into_body().collect())
			.await
			.map_err(|_| MgmtClientError::Timeout("read"))?
			.map_err(|e| MgmtClientError::Io(std::io::Error::other(e.to_string())))?
			.to_bytes();
		if status != StatusCode::OK {
			let body = String::from_utf8_lossy(&resp_body).into_owned();
			return Err(MgmtClientError::Http { status: status.as_u16(), body });
		}
		let response: Response = serde_json::from_slice(&resp_body).map_err(MgmtClientError::Decode)?;
		match response.outcome {
			ResponseOutcome::Result { result } => {
				serde_json::from_value(result).map_err(MgmtClientError::Decode)
			}
			ResponseOutcome::Error { error } => Err(MgmtClientError::Server(error)),
			ResponseOutcome::Event { .. } | ResponseOutcome::End { .. } => Err(MgmtClientError::Server(
				WireError::new(WireErrorKind::Internal, "received streaming frame on one-shot call"),
			)),
		}
	}

	/// Streaming verb call. Invokes `on_event` for each `Event` frame,
	/// returns when the server emits `End`, the connection drops, or
	/// the server emits `Error`.
	///
	/// # Errors
	/// Same shape as [`Self::call`], plus the streaming-specific case
	/// where the server emits a `Result` frame on what should be a
	/// streaming verb.
	pub async fn stream<A, F>(
		&self,
		verb: &str,
		args: &A,
		mut on_event: F,
	) -> Result<(), MgmtClientError>
	where
		A: serde::Serialize,
		F: FnMut(serde_json::Value),
	{
		let req = Request {
			id: 1,
			verb: verb.to_string(),
			args: serde_json::to_value(args).map_err(MgmtClientError::Encode)?,
		};
		let body_bytes = Bytes::from(serde_json::to_vec(&req).map_err(MgmtClientError::Encode)?);
		let resp = self.send(body_bytes).await?;
		let status = resp.status();
		if status != StatusCode::OK {
			let body = resp
				.into_body()
				.collect()
				.await
				.map_err(|e| MgmtClientError::Io(std::io::Error::other(e.to_string())))?
				.to_bytes();
			let body = String::from_utf8_lossy(&body).into_owned();
			return Err(MgmtClientError::Http { status: status.as_u16(), body });
		}
		// Drain the chunked body into a line accumulator and dispatch
		// each complete `\n`-delimited Response frame as it lands.
		let mut body = resp.into_body();
		let mut buf: Vec<u8> = Vec::with_capacity(4096);
		loop {
			let frame = match body.frame().await {
				Some(Ok(f)) => f,
				Some(Err(e)) => {
					return Err(MgmtClientError::Io(std::io::Error::other(e.to_string())));
				}
				None => break,
			};
			let Ok(data) = frame.into_data() else {
				// Trailers / non-data frame — ignore for the NDJSON contract.
				continue;
			};
			buf.extend_from_slice(&data);
			while let Some(idx) = buf.iter().position(|b| *b == b'\n') {
				let line: Vec<u8> = buf.drain(..=idx).collect();
				let line = &line[..line.len() - 1]; // strip trailing '\n'
				if line.is_empty() {
					continue;
				}
				let response: Response = serde_json::from_slice(line).map_err(MgmtClientError::Decode)?;
				match response.outcome {
					ResponseOutcome::Event { event } => on_event(event),
					ResponseOutcome::End { .. } => return Ok(()),
					ResponseOutcome::Error { error } => return Err(MgmtClientError::Server(error)),
					ResponseOutcome::Result { .. } => {
						return Err(MgmtClientError::Server(WireError::new(
							WireErrorKind::Internal,
							"received one-shot Result on streaming call",
						)));
					}
				}
			}
		}
		// Server closed mid-stream without an End frame — treat as
		// graceful EOF, mirroring `UnixMgmtClient::call_stream`.
		Ok(())
	}

	/// Open a fresh TCP connection, run the H1 handshake, send the
	/// POST, and return the response head + an in-flight body. The
	/// caller drains the body before going out of scope; the spawned
	/// driver task ends when the connection closes.
	async fn send(&self, body: Bytes) -> Result<hyper::Response<Incoming>, MgmtClientError> {
		let stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(self.addr))
			.await
			.map_err(|_| MgmtClientError::Timeout("connect"))??;
		let io = TokioIo::new(stream);
		let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
			.await
			.map_err(|e| MgmtClientError::Io(std::io::Error::other(e.to_string())))?;
		// Drive the connection to completion in the background. The
		// task ends when `sender` is dropped (which happens after this
		// function returns and the caller drops the response body),
		// or when the server closes the connection.
		tokio::spawn(async move {
			if let Err(e) = conn.await {
				tracing::debug!(?e, "mgmt http client connection ended");
			}
		});

		let mut builder = http::Request::builder()
			.method(Method::POST)
			.uri("/")
			.header(HOST, self.addr.to_string())
			.header(CONTENT_TYPE, "application/json");
		if let Some(token) = &self.token {
			builder = builder.header(AUTHORIZATION, format!("Bearer {token}"));
		}
		let http_req = builder
			.body(Full::new(body))
			.map_err(|e| MgmtClientError::Io(std::io::Error::other(e.to_string())))?;
		sender
			.send_request(http_req)
			.await
			.map_err(|e| MgmtClientError::Io(std::io::Error::other(e.to_string())))
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::protocol::{EndMarker, ResponseOutcome};

	/// Verify the line-accumulator splits a single byte buffer of
	/// concatenated NDJSON frames into individual `Response` decodes.
	/// This covers the streaming hot path without needing a real server.
	#[test]
	fn ndjson_line_accumulator_splits_frames() {
		let frames = vec![
			Response { id: 1, outcome: ResponseOutcome::Event { event: serde_json::json!({"i": 1}) } },
			Response { id: 1, outcome: ResponseOutcome::Event { event: serde_json::json!({"i": 2}) } },
			Response { id: 1, outcome: ResponseOutcome::End { end: EndMarker::default() } },
		];
		let mut wire: Vec<u8> = Vec::new();
		for f in &frames {
			wire.extend(serde_json::to_vec(f).unwrap());
			wire.push(b'\n');
		}
		// Simulate the drain loop from `stream` against the wire bytes.
		let mut buf = wire;
		let mut decoded: Vec<Response> = Vec::new();
		while let Some(idx) = buf.iter().position(|b| *b == b'\n') {
			let line: Vec<u8> = buf.drain(..=idx).collect();
			let body = &line[..line.len() - 1];
			let r: Response = serde_json::from_slice(body).unwrap();
			decoded.push(r);
		}
		assert_eq!(decoded.len(), 3);
		assert!(matches!(decoded[0].outcome, ResponseOutcome::Event { .. }));
		assert!(matches!(decoded[2].outcome, ResponseOutcome::End { .. }));
	}

	#[test]
	fn ndjson_line_accumulator_buffers_partial_frame_until_newline() {
		// Split one Response across two byte chunks at an arbitrary
		// internal offset to confirm the accumulator stitches them.
		let frame =
			Response { id: 7, outcome: ResponseOutcome::Result { result: serde_json::json!(42) } };
		let mut wire = serde_json::to_vec(&frame).unwrap();
		wire.push(b'\n');
		let (a, b) = wire.split_at(5);
		let mut buf: Vec<u8> = Vec::new();
		buf.extend_from_slice(a);
		assert!(!buf.contains(&b'\n'), "no complete frame yet");
		buf.extend_from_slice(b);
		let idx = buf.iter().position(|x| *x == b'\n').unwrap();
		let line: Vec<u8> = buf.drain(..=idx).collect();
		let body = &line[..line.len() - 1];
		let r: Response = serde_json::from_slice(body).unwrap();
		assert_eq!(r.id, 7);
	}
}
