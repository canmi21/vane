//! Unix-socket accept loop + per-connection line-delimited JSON
//! dispatch to verb handlers. The HTTP-over-TCP transport
//! ([`crate::http_server`]) speaks the same frame shapes, so dispatch
//! logic is shared.

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

/// Hard cap on the size of one NDJSON request line. Aligns with the
/// 1 MiB body cap on the HTTP transport so both faces of the mgmt
/// plane reject the same magnitude of oversized input. A real verb
/// payload is well under a kilobyte; anything larger is either a
/// malformed framing or an adversarial slowloris-by-line attack.
pub const MAX_NDJSON_LINE_BYTES: usize = 1024 * 1024;

/// Server-side dispatcher. Callers implement this against their own
/// application state and pass an `Arc<H>` to [`spawn_unix_server`] or
/// [`crate::spawn_http_server`].
#[async_trait]
pub trait Handler: Send + Sync + 'static {
	/// Dispatch a parsed request to either a one-shot result or a
	/// streaming event source. The server frames whichever outcome the
	/// handler returns.
	async fn dispatch(&self, req: Request) -> DispatchOutcome;
}

/// What `dispatch` returns. One-shot verbs (`ping`, `stats`, ...)
/// produce a single result/error frame; streaming verbs
/// (`tail_flow`) produce a sequence of `Event` frames terminated
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

/// RAII guard that tightens the process's file-mode-creation mask
/// (`umask`) for the duration of a scope and restores the prior value
/// on drop. Used around `UnixListener::bind` so a permissive operator
/// umask cannot widen the perms of the freshly-created socket file.
///
/// `umask(2)` is process-global, so any concurrent file creation in
/// other tasks while this guard is alive will also see the tightened
/// mask. For mgmt-socket bind this window is sub-millisecond and we
/// hold no other I/O off the critical path.
struct UmaskRestore {
	prev: libc::mode_t,
}

impl UmaskRestore {
	#[allow(unsafe_code)] // libc::umask is FFI; thread-safe POSIX call with no preconditions.
	fn tighten(mask: libc::mode_t) -> Self {
		// SAFETY: `umask` is a thread-safe POSIX call with no
		// preconditions. The return value is the previous mask.
		let prev = unsafe { libc::umask(mask) };
		Self { prev }
	}
}

impl Drop for UmaskRestore {
	#[allow(unsafe_code)] // libc::umask is FFI; see `tighten`.
	fn drop(&mut self) {
		// SAFETY: see `tighten`. Restoration is best-effort: there is
		// nothing useful to do if the kernel rejects the value (it
		// cannot — `umask` accepts any `mode_t`).
		unsafe {
			libc::umask(self.prev);
		}
	}
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

	// Tighten the inherited umask BEFORE bind so the kernel creates
	// the socket file with restrictive perms (0o660 modulo umask =
	// 0o600). Restore the previous umask via RAII regardless of bind
	// outcome so the daemon's other I/O paths see their original
	// settings.
	let _umask_restore = UmaskRestore::tighten(0o117);

	let listener = UnixListener::bind(socket_path)?;

	// Belt-and-suspenders: fchmod the socket to 0600 explicitly. The
	// umask path covers the bind-side race; this covers operators
	// running with permissive umasks (`077`) where the kernel would
	// have created the socket more permissively than we want.
	let perms = std::fs::Permissions::from_mode(0o600);
	std::fs::set_permissions(socket_path, perms)?;

	// Best-effort: warn when the socket's parent directory is more
	// permissive than the operator probably intends. A 0o755 parent
	// dir means any local user can `stat` the socket; a 0o777 parent
	// can unlink it. Both are footguns on multi-tenant hosts.
	if let Some(parent) = socket_path.parent()
		&& let Ok(meta) = std::fs::metadata(parent)
	{
		let mode = meta.permissions().mode() & 0o777;
		if mode != 0o700 && mode != 0o770 {
			tracing::warn!(
				dir = %parent.display(),
				mode = format!("{:#o}", mode),
				"mgmt socket parent dir is broader than 0700/0770; restrict perms or move the socket",
			);
		}
	}

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
					// Each per-connection driver gets a child token so
					// shutdown drives every in-flight verb / stream to
					// exit cleanly instead of leaving them blocked on
					// the read side of the socket.
					let conn_cancel = cancel.child_token();
					tokio::spawn(async move {
						let (read, write) = stream.into_split();
						handle_conn(read, write, h, conn_cancel).await;
					});
				}
			}
		}
	});
	Ok(handle)
}

/// Read a single NDJSON line with a hard byte cap. Returns `Ok(None)`
/// on clean EOF; `Ok(Some(_))` with a populated buffer when a line
/// terminator is seen; and the dedicated [`std::io::ErrorKind::FileTooLarge`]
/// when the cap is exceeded before a newline arrives.
async fn read_line_bounded<R>(
	reader: &mut BufReader<R>,
	buf: &mut String,
	cap: usize,
) -> std::io::Result<Option<()>>
where
	R: AsyncRead + Unpin,
{
	buf.clear();
	let start_len = buf.len();
	loop {
		let prev_len = buf.len();
		let n = reader.read_line(buf).await?;
		if n == 0 {
			// Clean EOF; return None if nothing buffered, else propagate
			// whatever the peer flushed without a trailing newline.
			return if buf.len() == start_len { Ok(None) } else { Ok(Some(())) };
		}
		// Strip the trailing newline so callers don't need to.
		if buf.ends_with('\n') {
			buf.pop();
			if buf.ends_with('\r') {
				buf.pop();
			}
			// Cap-check on the post-strip length so a single huge
			// read that includes the terminator still fails closed.
			if buf.len() > cap {
				return Err(std::io::Error::new(
					std::io::ErrorKind::InvalidData,
					format!("ndjson line exceeded {cap}-byte cap"),
				));
			}
			return Ok(Some(()));
		}
		// No newline read yet — bail if we'd exceed the per-line cap
		// before the peer flushes a terminator.
		if buf.len() > cap {
			return Err(std::io::Error::new(
				std::io::ErrorKind::InvalidData,
				format!("ndjson line exceeded {cap}-byte cap"),
			));
		}
		// Keep going — `read_line` can chunk on internal-buffer
		// boundaries even when more bytes are inbound.
		if buf.len() == prev_len + n && n == 0 {
			return Ok(Some(()));
		}
	}
}

/// Generic request loop, abstract over the read/write halves so unit
/// tests can drive it with `tokio::io::duplex` instead of a real Unix
/// socket. Production callers always pass the halves of a
/// [`tokio::net::UnixStream`].
pub(crate) async fn handle_conn<R, W, H>(
	read: R,
	mut write: W,
	handler: Arc<H>,
	cancel: CancellationToken,
) where
	R: AsyncRead + Unpin,
	W: AsyncWrite + Unpin,
	H: Handler,
{
	let mut reader = BufReader::new(read);
	let mut line = String::new();
	loop {
		// Select against the per-connection cancel token so a server-
		// wide shutdown drives every blocked read off the socket
		// instead of leaving the driver parked on `read_line`.
		let read_outcome = tokio::select! {
			biased;
			() = cancel.cancelled() => return,
			res = read_line_bounded(&mut reader, &mut line, MAX_NDJSON_LINE_BYTES) => res,
		};
		match read_outcome {
			Ok(None) => return,
			Ok(Some(())) => {}
			Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
				// Oversized line: write a structured error and close —
				// don't keep reading on a session whose framing is
				// already off the rails.
				let frame = Response {
					id: 0,
					outcome: ResponseOutcome::Error {
						error: WireError::new(WireErrorKind::BadArgs, format!("line too long: {e}")),
					},
				};
				let _ = write_frame(&mut write, &frame).await;
				return;
			}
			Err(e) => {
				tracing::debug!(?e, "mgmt read failed");
				return;
			}
		}
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
						// this socket. Cancel-on-shutdown drives every
						// `next_event` off so a daemon-wide stop trip
						// flushes an `End` frame and unblocks the client.
						loop {
							tokio::select! {
								biased;
								() = cancel.cancelled() => {
									let end = Response {
										id,
										outcome: ResponseOutcome::End { end: EndMarker::default() },
									};
									let _ = write_frame(&mut write, &end).await;
									return;
								}
								maybe = stream.next_event() => {
									let Some(event) = maybe else {
										let end = Response {
											id,
											outcome: ResponseOutcome::End { end: EndMarker::default() },
										};
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
				}
			}
			Err(e) => {
				let frame = Response {
					// id is unknown when the frame fails to parse — `0` is
					// the documented sentinel for "no correlation possible".
					id: 0,
					outcome: ResponseOutcome::Error {
						error: WireError::new(WireErrorKind::BadArgs, format!("parse: {e}")),
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
				_ => Err(WireError::new(WireErrorKind::UnknownVerb, format!("unknown {}", req.verb))),
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
		let server_task = tokio::spawn(handle_conn(c2s_r, s2c_w, handler, CancellationToken::new()));
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
	async fn server_rejects_line_exceeding_cap_with_bad_args() {
		let handler = Arc::new(StubHandler { last_verb: Mutex::new(None) });
		// Synthesise a line longer than MAX_NDJSON_LINE_BYTES with no
		// embedded newline. `read_line_bounded` must abort the
		// connection with a `BadArgs` frame and not let the request
		// reach the dispatcher.
		let huge_line = format!(
			"{{\"id\":1,\"verb\":\"x\",\"args\":\"{}\"}}\n",
			"A".repeat(MAX_NDJSON_LINE_BYTES + 1)
		);
		let bytes = drive(handler.clone(), &huge_line).await;
		let responses = parse_responses(&bytes);
		assert_eq!(responses.len(), 1);
		match &responses[0].outcome {
			ResponseOutcome::Error { error } => {
				assert_eq!(error.kind, WireErrorKind::BadArgs);
				assert!(error.message.contains("line too long"), "{}", error.message);
			}
			other => panic!("expected BadArgs error, got {other:?}"),
		}
		// Dispatcher never saw the request: handler's last_verb still None.
		assert!(handler.last_verb.lock().unwrap().is_none());
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
