//! HTTP-over-TCP management transport.
//!
//! Plaintext HTTP/1.1 only — TLS for the management endpoint is the
//! operator's concern, layered as a vane reverse-proxy rule per
//! `spec/architecture/10-management.md` § _Auth model_ /
//! _Recommended deployment_.
//!
//! Wire shape:
//! - request: `POST /` with a JSON body matching [`Request`]; any other
//!   method or path returns `405` / `404`.
//! - one-shot reply: `200 OK` + `Content-Type: application/json` + a
//!   single [`Response`] body.
//! - streaming reply: `200 OK` + `Content-Type: application/x-ndjson` +
//!   one JSON [`Response`] frame per chunk, terminated by an `End`
//!   frame. The client cancels by closing the TCP connection.
//!
//! Auth: `Authorization: Bearer <token>`, constant-time compared
//! against the configured token. Boot validation lives in the daemon.

use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited, combinators::BoxBody};
use hyper::body::{Body, Frame, Incoming};
use hyper::header::{AUTHORIZATION, CONTENT_TYPE, WWW_AUTHENTICATE};
use hyper::service::service_fn;
use hyper::{HeaderMap, Method, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::protocol::{EndMarker, Request, Response, ResponseOutcome, encode_line};
use crate::server::{DispatchOutcome, Handler};

/// Hard cap on request body size. Mgmt requests are tiny (a verb +
/// arg blob); 1 MiB is generous and lets us reject pathological clients
/// before they pin RAM.
const MAX_REQUEST_BODY_BYTES: usize = 1024 * 1024;

/// Channel depth for streaming responses. Each slot holds one already-
/// encoded NDJSON frame; backpressure flows naturally from a slow client
/// (TCP buffer fills → hyper stops draining → channel fills → producer
/// awaits). Mirrors the broadcast capacities chosen for `tail_flow_log`
/// in `spec/architecture/10-management.md` § _Streaming verb lifecycle_.
const STREAM_CHANNEL_DEPTH: usize = 64;

#[derive(Clone, Debug)]
pub struct HttpServerConfig {
	/// Bind addresses derived from `VANE_MGMT_HTTP_PORT` /
	/// `VANE_MGMT_HTTP_PUBLIC` / `VANE_BIND_IPV*`. Empty = HTTP transport
	/// disabled; the daemon should not call [`spawn_http_server`] in
	/// that case.
	pub binds: Vec<SocketAddr>,
	/// `Some(token)` enforces bearer auth; `None` means the operator
	/// opted into anonymous access (only legal on loopback per spec —
	/// the daemon validates that combination at boot).
	pub bearer_token: Option<Arc<str>>,
}

#[derive(thiserror::Error, Debug)]
pub enum HttpServerError {
	#[error("management http: bind {addr} failed: {source}")]
	Bind { addr: SocketAddr, source: std::io::Error },
}

/// Spawn one accept loop per bind address. Returns the spawned task
/// handles; each task runs until `cancel` fires or the listener errors
/// fatally.
///
/// # Errors
/// On the first bind failure, returns the error and aborts any tasks
/// spawned for earlier (already-bound) addresses so the daemon does not
/// end up serving a partial bind set.
pub async fn spawn_http_server<H: Handler>(
	cfg: HttpServerConfig,
	handler: Arc<H>,
	cancel: CancellationToken,
) -> Result<Vec<JoinHandle<()>>, HttpServerError> {
	let mut tasks: Vec<JoinHandle<()>> = Vec::with_capacity(cfg.binds.len());
	for addr in &cfg.binds {
		let listener = match TcpListener::bind(addr).await {
			Ok(l) => l,
			Err(source) => {
				// Roll back any earlier successful binds. Cancellation
				// is the contract; we honor it for partial-failure too.
				for t in &tasks {
					t.abort();
				}
				return Err(HttpServerError::Bind { addr: *addr, source });
			}
		};
		let handler = Arc::clone(&handler);
		let cancel = cancel.clone();
		let token = cfg.bearer_token.clone();
		let bind_addr = *addr;
		tasks.push(tokio::spawn(async move {
			run_accept_loop(listener, handler, token, cancel, bind_addr).await;
		}));
	}
	Ok(tasks)
}

async fn run_accept_loop<H: Handler>(
	listener: TcpListener,
	handler: Arc<H>,
	token: Option<Arc<str>>,
	cancel: CancellationToken,
	bind_addr: SocketAddr,
) {
	tracing::info!(%bind_addr, auth = if token.is_some() { "bearer" } else { "anonymous" }, "mgmt http listening");
	loop {
		tokio::select! {
			biased;
			() = cancel.cancelled() => return,
			res = listener.accept() => {
				let (stream, peer) = match res {
					Ok(v) => v,
					Err(e) => {
						tracing::debug!(?e, %bind_addr, "mgmt http accept error");
						continue;
					}
				};
				let handler = Arc::clone(&handler);
				let token = token.clone();
				tokio::spawn(async move {
					let io = TokioIo::new(stream);
					let svc = service_fn(move |req| {
						let handler = Arc::clone(&handler);
						let token = token.clone();
						async move { handle_request(req, handler, token, peer).await }
					});
					if let Err(e) = hyper::server::conn::http1::Builder::new()
						.serve_connection(io, svc)
						.await
					{
						tracing::debug!(?e, %peer, "mgmt http connection ended");
					}
				});
			}
		}
	}
}

type RespBody = BoxBody<Bytes, std::io::Error>;

async fn handle_request<H: Handler>(
	req: hyper::Request<Incoming>,
	handler: Arc<H>,
	token: Option<Arc<str>>,
	_peer: SocketAddr,
) -> Result<hyper::Response<RespBody>, std::convert::Infallible> {
	// Method / path gating happens before auth so a misrouted client
	// gets a deterministic 4xx instead of an auth failure that masks
	// the real problem.
	if req.uri().path() != "/" {
		return Ok(simple_status(StatusCode::NOT_FOUND));
	}
	if req.method() != Method::POST {
		return Ok(simple_status(StatusCode::METHOD_NOT_ALLOWED));
	}
	if let Some(expected) = &token
		&& !verify_bearer(req.headers(), expected)
	{
		return Ok(unauthorized());
	}
	let body_bytes = match read_request_body(req.into_body()).await {
		Ok(b) => b,
		Err(BodyReadError::TooLarge) => {
			return Ok(text_status(
				StatusCode::PAYLOAD_TOO_LARGE,
				"request body exceeds management transport limit",
			));
		}
		Err(BodyReadError::Io(e)) => {
			return Ok(text_status(StatusCode::BAD_REQUEST, &format!("body read failed: {e}")));
		}
	};
	let request = match serde_json::from_slice::<Request>(&body_bytes) {
		Ok(r) => r,
		Err(e) => return Ok(text_status(StatusCode::BAD_REQUEST, &format!("json parse: {e}"))),
	};
	let id = request.id;
	match handler.dispatch(request).await {
		DispatchOutcome::OneShot(Ok(value)) => {
			Ok(oneshot_response(&Response { id, outcome: ResponseOutcome::Result { result: value } }))
		}
		DispatchOutcome::OneShot(Err(error)) => {
			Ok(oneshot_response(&Response { id, outcome: ResponseOutcome::Error { error } }))
		}
		DispatchOutcome::Stream(stream) => Ok(streaming_response(id, stream)),
	}
}

/// Constant-time bearer-token check.
///
/// `subtle::ConstantTimeEq` runs in time independent of where the
/// mismatch is, defeating timing-side-channel guesses against the
/// token. A length mismatch short-circuits to `false` but still touches
/// the expected slice once so the call shape stays uniform across
/// equal- and unequal-length inputs.
fn verify_bearer(headers: &HeaderMap, expected: &Arc<str>) -> bool {
	use subtle::ConstantTimeEq;
	let Some(value) = headers.get(AUTHORIZATION) else {
		return false;
	};
	let Ok(s) = value.to_str() else { return false };
	let Some(token) = s.strip_prefix("Bearer ") else { return false };
	let exp = expected.as_bytes();
	let got = token.as_bytes();
	if exp.len() != got.len() {
		// Touch the expected slice so the early-exit branch still does
		// the same work as a length-equal compare (defence in depth —
		// the length is recoverable from network framing anyway, but
		// keep the codepath uniform).
		let _ = exp.ct_eq(exp);
		return false;
	}
	bool::from(exp.ct_eq(got))
}

enum BodyReadError {
	TooLarge,
	Io(String),
}

async fn read_request_body(body: Incoming) -> Result<Bytes, BodyReadError> {
	let limited = Limited::new(body, MAX_REQUEST_BODY_BYTES);
	match limited.collect().await {
		Ok(c) => Ok(c.to_bytes()),
		Err(e) => {
			// `Limited` boxes the underlying error; we discriminate
			// "too large" from "io" by downcasting to `LengthLimitError`.
			if e.downcast_ref::<http_body_util::LengthLimitError>().is_some() {
				Err(BodyReadError::TooLarge)
			} else {
				Err(BodyReadError::Io(e.to_string()))
			}
		}
	}
}

fn oneshot_response(frame: &Response) -> hyper::Response<RespBody> {
	let body_bytes = match serde_json::to_vec(frame) {
		Ok(b) => Bytes::from(b),
		Err(e) => {
			tracing::error!(?e, "mgmt http oneshot encode failed");
			return text_status(StatusCode::INTERNAL_SERVER_ERROR, "encode failed");
		}
	};
	build_response(StatusCode::OK, "application/json", full_body(body_bytes))
}

fn streaming_response(
	id: u64,
	mut stream: Box<dyn crate::server::EventStream + Send>,
) -> hyper::Response<RespBody> {
	// Channel decouples the stream producer task from hyper's body
	// poll loop. When hyper drops the body (client disconnect or
	// connection error) the receiver drops, the next `tx.send` fails,
	// and the producer task terminates — which drops `stream`,
	// triggering the EventStream's own cleanup. That is the
	// cancellation contract documented in
	// `spec/architecture/10-management.md` § _Streaming verb lifecycle_.
	let (tx, rx) = mpsc::channel::<Bytes>(STREAM_CHANNEL_DEPTH);
	tokio::spawn(async move {
		loop {
			let Some(event) = stream.next_event().await else {
				let end = Response { id, outcome: ResponseOutcome::End { end: EndMarker::default() } };
				if let Ok(bytes) = encode_line(&end) {
					let _ = tx.send(Bytes::from(bytes)).await;
				}
				return;
			};
			let frame = Response { id, outcome: ResponseOutcome::Event { event } };
			let bytes = match encode_line(&frame) {
				Ok(b) => Bytes::from(b),
				Err(e) => {
					tracing::error!(?e, id, "mgmt http stream encode failed");
					return;
				}
			};
			if tx.send(bytes).await.is_err() {
				// Client disconnected; drop the stream and exit.
				return;
			}
		}
	});
	let body = ChannelBody { rx }.boxed();
	build_response(StatusCode::OK, "application/x-ndjson", body)
}

struct ChannelBody {
	rx: mpsc::Receiver<Bytes>,
}

impl Body for ChannelBody {
	type Data = Bytes;
	type Error = std::io::Error;

	fn poll_frame(
		mut self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
		match self.rx.poll_recv(cx) {
			Poll::Ready(Some(b)) => Poll::Ready(Some(Ok(Frame::data(b)))),
			Poll::Ready(None) => Poll::Ready(None),
			Poll::Pending => Poll::Pending,
		}
	}
}

fn build_response(
	status: StatusCode,
	content_type: &'static str,
	body: RespBody,
) -> hyper::Response<RespBody> {
	let mut resp = hyper::Response::new(body);
	*resp.status_mut() = status;
	resp.headers_mut().insert(CONTENT_TYPE, content_type.parse().expect("static content type"));
	resp
}

fn full_body(bytes: Bytes) -> RespBody {
	Full::new(bytes).map_err(|never: std::convert::Infallible| match never {}).boxed()
}

fn simple_status(status: StatusCode) -> hyper::Response<RespBody> {
	let mut resp = hyper::Response::new(full_body(Bytes::new()));
	*resp.status_mut() = status;
	resp
}

fn text_status(status: StatusCode, body: &str) -> hyper::Response<RespBody> {
	let mut resp = hyper::Response::new(full_body(Bytes::copy_from_slice(body.as_bytes())));
	*resp.status_mut() = status;
	resp
		.headers_mut()
		.insert(CONTENT_TYPE, "text/plain; charset=utf-8".parse().expect("static content type"));
	resp
}

fn unauthorized() -> hyper::Response<RespBody> {
	let mut resp = simple_status(StatusCode::UNAUTHORIZED);
	resp.headers_mut().insert(WWW_AUTHENTICATE, "Bearer".parse().expect("static auth scheme"));
	resp
}

#[cfg(test)]
mod tests {
	use super::*;

	fn header_map(values: &[(hyper::header::HeaderName, &str)]) -> HeaderMap {
		let mut h = HeaderMap::new();
		for (name, val) in values {
			h.insert(name.clone(), val.parse().expect("valid header"));
		}
		h
	}

	#[test]
	fn verify_bearer_accepts_correct_token() {
		let token: Arc<str> = "s3cret".into();
		let headers = header_map(&[(AUTHORIZATION, "Bearer s3cret")]);
		assert!(verify_bearer(&headers, &token));
	}

	#[test]
	fn verify_bearer_rejects_wrong_token() {
		let token: Arc<str> = "s3cret".into();
		let headers = header_map(&[(AUTHORIZATION, "Bearer wrongx")]);
		assert!(!verify_bearer(&headers, &token));
	}

	#[test]
	fn verify_bearer_rejects_missing_header() {
		let token: Arc<str> = "s3cret".into();
		let headers = HeaderMap::new();
		assert!(!verify_bearer(&headers, &token));
	}

	#[test]
	fn verify_bearer_rejects_non_bearer_scheme() {
		let token: Arc<str> = "s3cret".into();
		let headers = header_map(&[(AUTHORIZATION, "Basic dXNlcjpwYXNz")]);
		assert!(!verify_bearer(&headers, &token));
	}

	#[test]
	fn verify_bearer_rejects_length_mismatch_without_panic() {
		// The length-mismatch branch must reject without panicking and
		// without leaking the prefix-match boundary via early return.
		let token: Arc<str> = "s3cret".into();
		let headers = header_map(&[(AUTHORIZATION, "Bearer s3")]);
		assert!(!verify_bearer(&headers, &token));
		let headers = header_map(&[(AUTHORIZATION, "Bearer s3cretextra")]);
		assert!(!verify_bearer(&headers, &token));
	}

	#[test]
	fn verify_bearer_rejects_empty_token_value() {
		let token: Arc<str> = "s3cret".into();
		let headers = header_map(&[(AUTHORIZATION, "Bearer ")]);
		assert!(!verify_bearer(&headers, &token));
	}
}
