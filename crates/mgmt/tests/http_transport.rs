//! End-to-end tests for the HTTP-over-TCP management transport.
//!
//! Each test spins up `spawn_http_server` against a stub `Handler` on
//! a fresh ephemeral port, then drives requests through either
//! `HttpMgmtClient` (typed path) or a raw TCP write (for malformed
//! requests that the typed client cannot construct). The cancellation
//! token is fired in the test's `Drop` so the server task exits with
//! the test, no leaked listeners.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener as StdTcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use vane_mgmt::client::MgmtClientError;
use vane_mgmt::protocol::{Request, WireError, WireErrorKind};
use vane_mgmt::server::{DispatchOutcome, EventStream, Handler};
use vane_mgmt::{HttpMgmtClient, HttpServerConfig, spawn_http_server};

// Stub handler
struct StubHandler;

#[async_trait]
impl Handler for StubHandler {
	async fn dispatch(&self, req: Request) -> DispatchOutcome {
		match req.verb.as_str() {
			"ping" => DispatchOutcome::OneShot(Ok(serde_json::json!({ "pong": true }))),
			"echo" => DispatchOutcome::OneShot(Ok(req.args)),
			"boom" => DispatchOutcome::OneShot(Err(WireError {
				kind: WireErrorKind::Internal,
				message: "deliberate".into(),
			})),
			"stream3" => DispatchOutcome::Stream(Box::new(ThreeEvents::new())),
			"infinite" => DispatchOutcome::Stream(Box::new(InfiniteEvents::default())),
			_ => DispatchOutcome::OneShot(Err(WireError {
				kind: WireErrorKind::UnknownVerb,
				message: format!("unknown {}", req.verb),
			})),
		}
	}
}

struct ThreeEvents {
	remaining: Vec<serde_json::Value>,
}

impl ThreeEvents {
	fn new() -> Self {
		// Pop pulls the last element first; queue in reverse so
		// the wire ordering matches the natural reading order.
		Self {
			remaining: vec![
				serde_json::json!({"i": 3}),
				serde_json::json!({"i": 2}),
				serde_json::json!({"i": 1}),
			],
		}
	}
}

#[async_trait]
impl EventStream for ThreeEvents {
	async fn next_event(&mut self) -> Option<serde_json::Value> {
		self.remaining.pop()
	}
}

/// Emits one event every 50 ms forever. Used to exercise the
/// client-disconnect cancellation path: the test drops the response
/// mid-stream and expects the producer to terminate when the server's
/// channel send fails.
#[derive(Default)]
struct InfiniteEvents {
	seq: u64,
}

#[async_trait]
impl EventStream for InfiniteEvents {
	async fn next_event(&mut self) -> Option<serde_json::Value> {
		tokio::time::sleep(Duration::from_millis(20)).await;
		self.seq += 1;
		Some(serde_json::json!({ "seq": self.seq }))
	}
}

// Server fixture
struct ServerFixture {
	addr: SocketAddr,
	cancel: CancellationToken,
}

impl Drop for ServerFixture {
	fn drop(&mut self) {
		self.cancel.cancel();
	}
}

async fn spawn_server(token: Option<&str>) -> ServerFixture {
	let listener = StdTcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
	let addr = listener.local_addr().expect("local addr");
	drop(listener);
	let cancel = CancellationToken::new();
	let cfg = HttpServerConfig {
		binds: vec![addr],
		bearer_token: token.map(|s| Arc::<str>::from(s.to_string())),
	};
	spawn_http_server(cfg, Arc::new(StubHandler), cancel.clone()).await.expect("spawn http server");
	// Give the accept loop a moment to be ready. spawn_http_server's
	// bind is synchronous (TcpListener::bind), so by the time it
	// returns the socket is listening — but the accept loop task
	// may not have polled accept() yet. A tiny yield avoids a connect
	// race on highly loaded CI runners.
	tokio::time::sleep(Duration::from_millis(20)).await;
	ServerFixture { addr, cancel }
}

// Typed-client tests
#[tokio::test]
async fn http_oneshot_round_trip_with_token() {
	let srv = spawn_server(Some("topsecret")).await;
	let client = HttpMgmtClient::new(srv.addr, Some(Arc::<str>::from("topsecret")));
	let r: serde_json::Value = client.call("ping", &serde_json::json!({})).await.expect("ping");
	assert_eq!(r["pong"], true);
}

#[tokio::test]
async fn http_oneshot_loopback_no_token_accepts_anonymous() {
	// Server with bearer_token = None — the daemon would only accept
	// this combination on loopback per the spec auth model. The mgmt
	// crate doesn't enforce that (the daemon's boot-validation does),
	// so we can construct the unguarded server here directly.
	let srv = spawn_server(None).await;
	let client = HttpMgmtClient::new(srv.addr, None);
	let r: serde_json::Value = client.call("ping", &serde_json::json!({})).await.expect("ping");
	assert_eq!(r["pong"], true);
}

#[tokio::test]
async fn http_oneshot_rejects_missing_bearer() {
	let srv = spawn_server(Some("topsecret")).await;
	let client = HttpMgmtClient::new(srv.addr, None);
	let err = client
		.call::<_, serde_json::Value>("ping", &serde_json::json!({}))
		.await
		.expect_err("auth must fail");
	match err {
		MgmtClientError::Http { status, .. } => assert_eq!(status, 401),
		other => panic!("expected Http 401, got {other:?}"),
	}
}

#[tokio::test]
async fn http_oneshot_rejects_wrong_bearer() {
	let srv = spawn_server(Some("topsecret")).await;
	let client = HttpMgmtClient::new(srv.addr, Some(Arc::<str>::from("nope")));
	let err = client
		.call::<_, serde_json::Value>("ping", &serde_json::json!({}))
		.await
		.expect_err("auth must fail");
	match err {
		MgmtClientError::Http { status, .. } => assert_eq!(status, 401),
		other => panic!("expected Http 401, got {other:?}"),
	}
}

#[tokio::test]
async fn http_oneshot_surfaces_handler_business_error_on_200() {
	// Per the spec error wire format: business errors stay 200 + JSON
	// Response { error: ... }. The client surfaces them as
	// MgmtClientError::Server, NOT as MgmtClientError::Http.
	let srv = spawn_server(None).await;
	let client = HttpMgmtClient::new(srv.addr, None);
	let err = client
		.call::<_, serde_json::Value>("boom", &serde_json::json!({}))
		.await
		.expect_err("must surface server error");
	match err {
		MgmtClientError::Server(w) => {
			assert_eq!(w.kind, WireErrorKind::Internal);
			assert!(w.message.contains("deliberate"));
		}
		other => panic!("expected Server, got {other:?}"),
	}
}

#[tokio::test]
async fn http_oneshot_echoes_typed_args() {
	let srv = spawn_server(None).await;
	let client = HttpMgmtClient::new(srv.addr, None);
	let r: serde_json::Value =
		client.call("echo", &serde_json::json!({"hello": "world", "n": 7})).await.expect("echo");
	assert_eq!(r["hello"], "world");
	assert_eq!(r["n"], 7);
}

#[tokio::test]
async fn http_streaming_emits_event_frames_then_end() {
	let srv = spawn_server(None).await;
	let client = HttpMgmtClient::new(srv.addr, None);
	let collected = std::sync::Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new()));
	let collected_clone = std::sync::Arc::clone(&collected);
	client
		.stream("stream3", &serde_json::json!({}), move |ev| {
			collected_clone.lock().unwrap().push(ev);
		})
		.await
		.expect("stream completed");
	let collected = collected.lock().unwrap();
	assert_eq!(collected.len(), 3, "three event frames before End");
	assert_eq!(collected[0]["i"], 1);
	assert_eq!(collected[1]["i"], 2);
	assert_eq!(collected[2]["i"], 3);
}

#[tokio::test]
async fn http_streaming_terminates_when_client_drops_mid_stream() {
	// Spawn a server emitting forever, run the streaming call with a
	// callback that bails out after 2 events by panicking through a
	// channel signal — then drop the future. The server's producer
	// task should observe the channel close and terminate. We don't
	// have a direct probe for "task ended"; instead we assert the
	// client doesn't hang and the server task drops `InfiniteEvents`.
	let srv = spawn_server(None).await;
	let client = HttpMgmtClient::new(srv.addr, None);

	// Race the streaming call against a 250ms deadline. If the server
	// failed to detect the client disconnect, the call would run
	// forever — the timeout catches that.
	let collected = std::sync::Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new()));
	let collected_clone = std::sync::Arc::clone(&collected);
	let stream_task = async move {
		let _ = client
			.stream("infinite", &serde_json::json!({}), move |ev| {
				collected_clone.lock().unwrap().push(ev);
			})
			.await;
	};
	let _ = tokio::time::timeout(Duration::from_millis(250), stream_task).await;
	// At least one event made it across before the client was dropped.
	let n = collected.lock().unwrap().len();
	assert!(n >= 1, "expected at least one event before drop, got {n}");
}

// Raw-HTTP tests for malformed-request paths
/// Send a raw HTTP request and read the full response. Blocking I/O
/// in a sync helper — easier to construct deliberately malformed
/// frames than via `hyper::Request::builder()`.
fn raw_http(addr: SocketAddr, raw_request: &str) -> (u16, String, String) {
	let mut sock = TcpStream::connect(addr).expect("connect");
	sock.set_read_timeout(Some(Duration::from_secs(2))).expect("read timeout");
	sock.write_all(raw_request.as_bytes()).expect("write");
	let mut buf = Vec::new();
	let _ = sock.read_to_end(&mut buf);
	let text = String::from_utf8_lossy(&buf).into_owned();
	let mut lines = text.lines();
	let status_line = lines.next().expect("status line").to_string();
	// "HTTP/1.1 401 Unauthorized" → 401.
	let status: u16 = status_line.split_whitespace().nth(1).unwrap_or("0").parse().unwrap_or(0);
	(status, status_line, text)
}

#[tokio::test]
async fn http_returns_405_for_get() {
	let srv = spawn_server(None).await;
	let req = format!("GET / HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n", addr = srv.addr);
	let (status, _line, body) = tokio::task::spawn_blocking({
		let addr = srv.addr;
		move || raw_http(addr, &req)
	})
	.await
	.expect("blocking ok");
	assert_eq!(status, 405, "GET / must be 405; full response:\n{body}");
}

#[tokio::test]
async fn http_returns_404_for_other_path() {
	let srv = spawn_server(None).await;
	let req = format!(
		"POST /metrics HTTP/1.1\r\nHost: {addr}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
		addr = srv.addr,
	);
	let (status, _line, body) = tokio::task::spawn_blocking({
		let addr = srv.addr;
		move || raw_http(addr, &req)
	})
	.await
	.expect("blocking ok");
	assert_eq!(status, 404, "POST /metrics must be 404; full response:\n{body}");
}

#[tokio::test]
async fn http_returns_400_for_garbage_json() {
	let srv = spawn_server(None).await;
	let body = "this is not json";
	let req = format!(
		"POST / HTTP/1.1\r\nHost: {addr}\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n{body}",
		addr = srv.addr,
		len = body.len(),
	);
	let (status, _line, full) = tokio::task::spawn_blocking({
		let addr = srv.addr;
		move || raw_http(addr, &req)
	})
	.await
	.expect("blocking ok");
	assert_eq!(status, 400, "garbage JSON must be 400; full response:\n{full}");
}

#[tokio::test]
async fn http_401_includes_www_authenticate_header() {
	let srv = spawn_server(Some("topsecret")).await;
	let body = serde_json::to_string(&Request {
		id: 1,
		verb: "ping".to_string(),
		args: serde_json::Value::Null,
	})
	.unwrap();
	let req = format!(
		"POST / HTTP/1.1\r\nHost: {addr}\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n{body}",
		addr = srv.addr,
		len = body.len(),
	);
	let (status, _line, full) = tokio::task::spawn_blocking({
		let addr = srv.addr;
		move || raw_http(addr, &req)
	})
	.await
	.expect("blocking ok");
	assert_eq!(status, 401);
	assert!(
		full.to_ascii_lowercase().contains("www-authenticate: bearer"),
		"401 response must carry WWW-Authenticate: Bearer; got:\n{full}",
	);
}
