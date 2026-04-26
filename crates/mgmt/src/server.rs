//! Management server: Unix-socket accept loop + per-connection
//! line-delimited JSON dispatch to verb handlers.
//!
//! Stage 1: Unix transport only. Streaming verbs (`tail_flow_log`,
//! `tail_log`) and HTTP-over-TCP transport land in S2. The framing
//! contract here is identical to what Stage 2 will speak, so the
//! server-side dispatch survives the transport change.
//!
//! See `spec/architecture/10-management.md`. Features: S1-24, S1-25.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::protocol::{
	EndMarker, Request, Response, ResponseOutcome, WireError, WireErrorKind, encode_line,
};

/// Server-side dispatcher. The daemon implements this against its own
/// state — graph swap, listener set, factories, shutdown trigger —
/// keeping `vane-mgmt` free of any engine dependency.
#[async_trait]
pub trait Handler: Send + Sync + 'static {
	/// Dispatch a parsed request to either a one-shot result or a
	/// streaming event source. The server frames whichever outcome the
	/// handler returns.
	async fn dispatch(&self, req: Request) -> DispatchOutcome;
}

/// What `dispatch` returns. One-shot verbs (`ping`, `stats`, ...)
/// produce a single result/error frame; streaming verbs
/// (`tail_flow_log`) produce a sequence of `Event` frames terminated
/// by an `End` frame.
pub enum DispatchOutcome {
	/// One-shot reply: a single JSON value or a structured error.
	OneShot(Result<serde_json::Value, WireError>),
	/// Streaming reply: each call to `next_event` yields the next
	/// `Event` payload, or `None` to terminate with an `End` frame.
	Stream(Box<dyn EventStream + Send>),
}

/// A streaming event source. The server polls `next_event` until the
/// client disconnects or the stream returns `None`.
#[async_trait]
pub trait EventStream: Send {
	/// `Some(event)` = next event payload to write as `Event { event }`.
	/// `None` = stream terminated normally; the server writes `End`.
	async fn next_event(&mut self) -> Option<serde_json::Value>;
}

/// Bind a Unix socket and serve mgmt requests until `cancel` fires.
///
/// On bind, an existing socket file at `socket_path` is unlinked first
/// — operators are responsible for ensuring no other `vaned` is using
/// the path. The socket file's mode is set to `0600`: mgmt access is
/// gated by file-system permissions only, no in-band auth.
///
/// On cancellation, the bound socket file is removed before the task
/// returns so a subsequent `vaned` boot can re-bind cleanly.
///
/// # Errors
/// Bind / chmod / remove-stale-file failures bubble up as
/// [`std::io::Error`].
pub async fn spawn_unix_server<H: Handler>(
	socket_path: &Path,
	handler: Arc<H>,
	cancel: CancellationToken,
) -> std::io::Result<JoinHandle<()>> {
	// Unlink any stale socket file before bind. systemd-style socket
	// activation is not supported this round.
	let _ = std::fs::remove_file(socket_path);
	let listener = UnixListener::bind(socket_path)?;

	let perms = std::fs::Permissions::from_mode(0o600);
	std::fs::set_permissions(socket_path, perms)?;

	let socket_path: PathBuf = socket_path.to_path_buf();
	let handle = tokio::spawn(async move {
		loop {
			tokio::select! {
				biased;
				() = cancel.cancelled() => {
					let _ = std::fs::remove_file(&socket_path);
					return;
				}
				accepted = listener.accept() => {
					let stream: UnixStream = match accepted {
						Ok((s, _)) => s,
						Err(e) => {
							tracing::warn!(?e, "mgmt accept failed");
							continue;
						}
					};
					let h = Arc::clone(&handler);
					tokio::spawn(async move {
						let (read, write) = stream.into_split();
						handle_conn(read, write, h).await;
					});
				}
			}
		}
	});
	Ok(handle)
}

/// Generic request loop, abstract over the read/write halves so unit
/// tests can drive it with `tokio::io::duplex` instead of a real Unix
/// socket. Production callers always pass the halves of a
/// [`tokio::net::UnixStream`].
pub(crate) async fn handle_conn<R, W, H>(read: R, mut write: W, handler: Arc<H>)
where
	R: AsyncRead + Unpin,
	W: AsyncWrite + Unpin,
	H: Handler,
{
	let mut lines = BufReader::new(read).lines();
	loop {
		let line = match lines.next_line().await {
			Ok(Some(l)) => l,
			Ok(None) => return,
			Err(e) => {
				tracing::debug!(?e, "mgmt read failed");
				return;
			}
		};
		if line.is_empty() {
			continue;
		}
		match serde_json::from_str::<Request>(&line) {
			Ok(req) => {
				let id = req.id;
				match handler.dispatch(req).await {
					DispatchOutcome::OneShot(Ok(value)) => {
						let frame = Response { id, outcome: ResponseOutcome::Result { result: value } };
						if write_frame(&mut write, &frame).await.is_err() {
							return;
						}
					}
					DispatchOutcome::OneShot(Err(error)) => {
						let frame = Response { id, outcome: ResponseOutcome::Error { error } };
						if write_frame(&mut write, &frame).await.is_err() {
							return;
						}
					}
					DispatchOutcome::Stream(mut stream) => {
						// Streaming verbs consume the connection — once we
						// start streaming we don't read more requests on
						// this socket. Client disconnects by closing the
						// socket; server detects that via write failure
						// or the read-side seeing EOF.
						loop {
							let Some(event) = stream.next_event().await else {
								let end =
									Response { id, outcome: ResponseOutcome::End { end: EndMarker::default() } };
								let _ = write_frame(&mut write, &end).await;
								return;
							};
							let frame = Response { id, outcome: ResponseOutcome::Event { event } };
							if write_frame(&mut write, &frame).await.is_err() {
								return;
							}
						}
					}
				}
			}
			Err(e) => {
				let frame = Response {
					// id is unknown when the frame fails to parse — `0` is
					// the documented sentinel for "no correlation possible".
					id: 0,
					outcome: ResponseOutcome::Error {
						error: WireError { kind: WireErrorKind::BadArgs, message: format!("parse: {e}") },
					},
				};
				if write_frame(&mut write, &frame).await.is_err() {
					return;
				}
			}
		}
	}
}

/// Encode a response and write it as one NDJSON line. Wraps the two
/// fallible sub-steps (encode → write) so the streaming loop has a
/// single error path.
async fn write_frame<W: AsyncWrite + Unpin>(
	write: &mut W,
	frame: &Response,
) -> Result<(), std::io::Error> {
	let bytes = match encode_line(frame) {
		Ok(b) => b,
		Err(e) => {
			tracing::error!(?e, "mgmt response encode failed");
			return Err(std::io::Error::other(e));
		}
	};
	write.write_all(&bytes).await
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::sync::Mutex;

	struct StubHandler {
		// Records the last verb seen, for assertions.
		last_verb: Mutex<Option<String>>,
	}

	#[async_trait]
	impl Handler for StubHandler {
		async fn dispatch(&self, req: Request) -> DispatchOutcome {
			*self.last_verb.lock().unwrap() = Some(req.verb.clone());
			let result: Result<serde_json::Value, WireError> = match req.verb.as_str() {
				"ping" => Ok(serde_json::json!({ "pong": true })),
				"echo" => Ok(req.args),
				"stream2" => {
					return DispatchOutcome::Stream(Box::new(MockStream::with_two_events()));
				}
				_ => Err(WireError {
					kind: WireErrorKind::UnknownVerb,
					message: format!("unknown {}", req.verb),
				}),
			};
			DispatchOutcome::OneShot(result)
		}
	}

	/// Trivial event stream: emits two events then terminates with `None`,
	/// modelling the smallest possible streaming verb.
	struct MockStream {
		remaining: Vec<serde_json::Value>,
	}

	impl MockStream {
		fn with_two_events() -> Self {
			// Pop returns the last element first; queue events in reverse
			// so the wire ordering observed by the client matches the
			// natural reading order (n=2 then n=1).
			Self { remaining: vec![serde_json::json!({ "n": 1 }), serde_json::json!({ "n": 2 })] }
		}
	}

	#[async_trait]
	impl EventStream for MockStream {
		async fn next_event(&mut self) -> Option<serde_json::Value> {
			self.remaining.pop()
		}
	}

	/// Pump one or more request lines through `handle_conn` against a
	/// stub handler, returning the response bytes the server wrote.
	async fn drive(handler: Arc<StubHandler>, requests: &str) -> Vec<u8> {
		// Client writes requests on `c2s_w` → server reads on `c2s_r`.
		// Server writes responses on `s2c_w` → client reads on `s2c_r`.
		let (c2s_r, mut c2s_w) = tokio::io::duplex(8192);
		let (s2c_w, mut s2c_r) = tokio::io::duplex(8192);
		let req = requests.to_string();
		let server_task = tokio::spawn(handle_conn(c2s_r, s2c_w, handler));
		c2s_w.write_all(req.as_bytes()).await.expect("write requests");
		// Closing the write half makes `next_line` return None on the
		// server side so the task completes cleanly.
		drop(c2s_w);
		server_task.await.expect("server task");
		// Read everything the server wrote.
		let mut buf = Vec::new();
		tokio::io::AsyncReadExt::read_to_end(&mut s2c_r, &mut buf).await.expect("read responses");
		buf
	}

	fn parse_responses(bytes: &[u8]) -> Vec<Response> {
		std::str::from_utf8(bytes)
			.expect("utf8")
			.lines()
			.filter(|l| !l.is_empty())
			.map(|l| serde_json::from_str(l).expect("parse response"))
			.collect()
	}

	#[tokio::test]
	async fn server_stub_dispatches_known_verb_and_writes_result_line() {
		let handler = Arc::new(StubHandler { last_verb: Mutex::new(None) });
		let req = Request { id: 11, verb: "ping".to_string(), args: serde_json::Value::Null };
		let raw = serde_json::to_string(&req).unwrap() + "\n";
		let bytes = drive(Arc::clone(&handler), &raw).await;
		let responses = parse_responses(&bytes);
		assert_eq!(responses.len(), 1);
		assert_eq!(responses[0].id, 11);
		match &responses[0].outcome {
			ResponseOutcome::Result { result } => assert_eq!(result["pong"], true),
			other => panic!("unexpected outcome: {other:?}"),
		}
		assert_eq!(handler.last_verb.lock().unwrap().as_deref(), Some("ping"));
	}

	#[tokio::test]
	async fn server_stub_writes_error_for_unknown_verb() {
		let handler = Arc::new(StubHandler { last_verb: Mutex::new(None) });
		let req = Request { id: 5, verb: "wat".to_string(), args: serde_json::Value::Null };
		let raw = serde_json::to_string(&req).unwrap() + "\n";
		let bytes = drive(handler, &raw).await;
		let responses = parse_responses(&bytes);
		assert_eq!(responses.len(), 1);
		assert_eq!(responses[0].id, 5);
		match &responses[0].outcome {
			ResponseOutcome::Error { error } => {
				assert_eq!(error.kind, WireErrorKind::UnknownVerb);
				assert!(error.message.contains("wat"));
			}
			other => panic!("expected error, got {other:?}"),
		}
	}

	#[tokio::test]
	async fn server_stub_writes_bad_args_error_for_unparseable_request() {
		let handler = Arc::new(StubHandler { last_verb: Mutex::new(None) });
		let raw = "this is not json\n";
		let bytes = drive(handler, raw).await;
		let responses = parse_responses(&bytes);
		assert_eq!(responses.len(), 1);
		// id must be the documented 0 sentinel — there's no parsed id to echo.
		assert_eq!(responses[0].id, 0);
		match &responses[0].outcome {
			ResponseOutcome::Error { error } => assert_eq!(error.kind, WireErrorKind::BadArgs),
			other => panic!("expected error, got {other:?}"),
		}
	}

	#[tokio::test]
	async fn server_dispatches_streaming_verb_writes_event_then_end() {
		let handler = Arc::new(StubHandler { last_verb: Mutex::new(None) });
		let req = Request { id: 99, verb: "stream2".to_string(), args: serde_json::Value::Null };
		let raw = serde_json::to_string(&req).unwrap() + "\n";
		let bytes = drive(handler, &raw).await;
		let responses = parse_responses(&bytes);
		// 2 events + 1 end = 3 frames, all carrying id=99.
		assert_eq!(responses.len(), 3, "two events plus a terminating End frame");
		for r in &responses {
			assert_eq!(r.id, 99, "streaming frames echo the request id");
		}
		assert!(matches!(responses[0].outcome, ResponseOutcome::Event { .. }));
		assert!(matches!(responses[1].outcome, ResponseOutcome::Event { .. }));
		assert!(matches!(responses[2].outcome, ResponseOutcome::End { .. }));
		// Exact event payloads in order.
		if let ResponseOutcome::Event { event } = &responses[0].outcome {
			assert_eq!(event["n"], 2);
		}
		if let ResponseOutcome::Event { event } = &responses[1].outcome {
			assert_eq!(event["n"], 1);
		}
	}

	#[tokio::test]
	async fn server_stub_handles_multiple_requests_serial_per_connection() {
		let handler = Arc::new(StubHandler { last_verb: Mutex::new(None) });
		let r1 =
			serde_json::to_string(&Request { id: 1, verb: "ping".into(), args: serde_json::Value::Null })
				.unwrap();
		let r2 = serde_json::to_string(&Request {
			id: 2,
			verb: "echo".into(),
			args: serde_json::json!({"x": 1}),
		})
		.unwrap();
		let r3 =
			serde_json::to_string(&Request { id: 3, verb: "nope".into(), args: serde_json::Value::Null })
				.unwrap();
		let raw = format!("{r1}\n{r2}\n\n{r3}\n");
		let bytes = drive(handler, &raw).await;
		let responses = parse_responses(&bytes);
		assert_eq!(responses.len(), 3, "blank line is skipped, not echoed back");
		assert_eq!(responses[0].id, 1);
		assert_eq!(responses[1].id, 2);
		assert_eq!(responses[2].id, 3);
		assert!(matches!(responses[0].outcome, ResponseOutcome::Result { .. }));
		assert!(matches!(responses[1].outcome, ResponseOutcome::Result { .. }));
		assert!(matches!(responses[2].outcome, ResponseOutcome::Error { .. }));
	}
}
