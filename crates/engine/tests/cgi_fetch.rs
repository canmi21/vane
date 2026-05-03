//! End-to-end tests for the CGI fetch driver.
//!
//! Each test:
//!
//! 1. Writes a tiny `/bin/sh` script as a `chmod 0o755` tempfile
//!    (the `tempbin` helper below).
//! 2. Builds the CGI args via `vane_engine::fetch::cgi::factory`.
//! 3. Extracts the resulting `Arc<dyn L7Fetch>` from the `FetchInst`.
//! 4. Invokes `.fetch(req, conn, ctx)` directly and asserts on the
//!    response status / headers / body.
//!
//! Direct invocation (rather than driving via a full listener / hyper
//! decoder) keeps the tests focused on the CGI path: env construction,
//! header parsing, exit-code mapping, stderr-to-tracing, etc.
//!
//! Spec anchor: `spec/architecture/15-cgi.md`.

#![allow(clippy::too_many_lines)]

use std::io::Write as _;
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt as _;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::BodyExt as _;
use parking_lot::Mutex;
use serde_json::{Value, json};
use tempfile::NamedTempFile;
use tokio_util::sync::CancellationToken;
use vane_core::{
	Body, ConnContext, ConnId, FlowCtx, FlowLogEvent, FlowLogSink, FlowLogVerbosity, L7Fetch,
	L7FetchOutput, Request, TrajectoryBuilder, Transport,
};
use vane_engine::flow_graph::FetchInst;

struct DropSink;
impl FlowLogSink for DropSink {
	fn emit(&self, _event: FlowLogEvent) {}
}

/// Write `script` to a chmod 0o755 tempfile and return it. The first
/// line is expected to be a `#!` shebang so `execve(2)` can load the
/// interpreter; for plain `/bin/sh` scripts that's `#!/bin/sh\n`.
fn tempbin(script: &str) -> NamedTempFile {
	let mut f = NamedTempFile::new().expect("tmp");
	f.write_all(script.as_bytes()).expect("write");
	std::fs::set_permissions(f.path(), std::fs::Permissions::from_mode(0o755)).expect("chmod");
	f
}

fn current_uid() -> u32 {
	use std::os::unix::fs::MetadataExt as _;
	let f = NamedTempFile::new().expect("probe tmp");
	std::fs::metadata(f.path()).expect("probe stat").uid()
}

fn current_gid() -> u32 {
	use std::os::unix::fs::MetadataExt as _;
	let f = NamedTempFile::new().expect("probe tmp");
	std::fs::metadata(f.path()).expect("probe stat").gid()
}

/// Minimal-valid args parameterised on the binary path. Tests further
/// override via `args["..."] = json!(...)`.
fn args_for(bin: &std::path::Path) -> Value {
	json!({
		"upstream_kind": "cgi",
		"binary": bin.to_str().unwrap(),
		"script_name": "/cgi-bin/app",
		"working_dir": bin.parent().unwrap().to_str().unwrap(),
		"env": {},
		"block_headers": ["Authorization", "Cookie", "Proxy-Authorization"],
		"security": {
			"uid": current_uid(),
			"gid": current_gid(),
			"limits": { "memory_mb": null, "cpu_seconds": null, "max_processes": null },
			"chroot": null,
		},
	})
}

fn build_fetch(args: &Value) -> Arc<dyn L7Fetch> {
	let inst = vane_engine::fetch::cgi::factory(args).expect("factory must accept");
	match inst {
		FetchInst::L7(f) => f,
		FetchInst::L4(_) => panic!("cgi factory must return L7"),
	}
}

fn make_conn() -> Arc<ConnContext> {
	let remote: SocketAddr = "127.0.0.1:54321".parse().unwrap();
	let local: SocketAddr = "127.0.0.1:8080".parse().unwrap();
	Arc::new(ConnContext {
		id: ConnId(1),
		remote,
		local,
		transport: Transport::Tcp,
		entered_at: std::time::Instant::now(),
		tls: parking_lot::Mutex::new(None),
		http_version: OnceLock::new(),
		user: parking_lot::Mutex::new(http::Extensions::new()),
	})
}

fn make_ctx(conn: &Arc<ConnContext>) -> FlowCtx {
	FlowCtx {
		span: tracing::Span::none(),
		log: Arc::new(DropSink) as Arc<dyn FlowLogSink>,
		cancel: CancellationToken::new(),
		verbosity: FlowLogVerbosity::Trajectory,
		trajectory: TrajectoryBuilder::new(conn.id, vane_core::NodeId::new(0), 0),
	}
}

fn build_request(method: &str, path: &str, body: Body) -> Request {
	http::Request::builder()
		.method(method)
		.uri(path)
		.header("host", "example.test")
		.body(body)
		.expect("build request")
}

async fn invoke(
	fetch: &Arc<dyn L7Fetch>,
	req: Request,
	conn: &Arc<ConnContext>,
) -> http::Response<Body> {
	let mut ctx = make_ctx(conn);
	let out = fetch.fetch(req, conn, &mut ctx).await.expect("fetch must not error to caller");
	match out {
		L7FetchOutput::Response(r) => r,
		L7FetchOutput::Tunnel(_) => panic!("cgi must produce Response, not Tunnel"),
	}
}

async fn collect_body(resp: http::Response<Body>) -> (http::StatusCode, http::HeaderMap, Bytes) {
	let (parts, body) = resp.into_parts();
	let bytes = body.collect().await.expect("collect body").to_bytes();
	(parts.status, parts.headers, bytes)
}

// ---------------------------------------------------------------------------
// 1. happy path
// ---------------------------------------------------------------------------

#[tokio::test]
async fn happy_path_writes_status_header_and_body() {
	let bin = tempbin(
		"#!/bin/sh\n\
		 printf 'Status: 200 OK\\r\\n'\n\
		 printf 'Content-Type: text/plain\\r\\n'\n\
		 printf 'X-Vane-Test: hello\\r\\n'\n\
		 printf '\\r\\n'\n\
		 printf 'happy-body'\n",
	);
	let fetch = build_fetch(&args_for(bin.path()));
	let conn = make_conn();
	let req = build_request("GET", "/cgi-bin/app", Body::Empty);
	let (status, headers, body) = collect_body(invoke(&fetch, req, &conn).await).await;
	assert_eq!(status, http::StatusCode::OK);
	assert_eq!(headers.get("X-Vane-Test").and_then(|v| v.to_str().ok()), Some("hello"));
	assert_eq!(
		headers.get(http::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()),
		Some("text/plain")
	);
	assert_eq!(body.as_ref(), b"happy-body");
}

// ---------------------------------------------------------------------------
// 2. stdin passthrough
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stdin_body_passes_through_to_child_and_back() {
	let bin = tempbin(
		"#!/bin/sh\n\
		 printf 'Status: 200 OK\\r\\n'\n\
		 printf '\\r\\n'\n\
		 cat\n",
	);
	let fetch = build_fetch(&args_for(bin.path()));
	let conn = make_conn();
	let payload = b"echo-this-back";
	let req = http::Request::builder()
		.method("POST")
		.uri("/cgi-bin/app")
		.header("host", "example.test")
		.header("content-length", payload.len().to_string())
		.body(Body::Static(Bytes::from_static(payload)))
		.expect("build POST");
	let (status, _headers, body) = collect_body(invoke(&fetch, req, &conn).await).await;
	assert_eq!(status, http::StatusCode::OK);
	assert_eq!(body.as_ref(), payload, "stdin body must reach the child + come back via stdout");
}

// ---------------------------------------------------------------------------
// 3. Status: 404 Not Found surfaces as 404
// ---------------------------------------------------------------------------

#[tokio::test]
async fn status_404_passes_through_to_client() {
	let bin = tempbin(
		"#!/bin/sh\n\
		 printf 'Status: 404 Not Found\\r\\n'\n\
		 printf '\\r\\n'\n\
		 printf 'gone'\n",
	);
	let fetch = build_fetch(&args_for(bin.path()));
	let conn = make_conn();
	let req = build_request("GET", "/cgi-bin/app", Body::Empty);
	let (status, _headers, body) = collect_body(invoke(&fetch, req, &conn).await).await;
	assert_eq!(status, http::StatusCode::NOT_FOUND);
	assert_eq!(body.as_ref(), b"gone");
}

// ---------------------------------------------------------------------------
// 4. Location: without Status: → 302
// ---------------------------------------------------------------------------

#[tokio::test]
async fn location_header_without_status_yields_302() {
	let bin = tempbin(
		"#!/bin/sh\n\
		 printf 'Location: /elsewhere\\r\\n'\n\
		 printf '\\r\\n'\n",
	);
	let fetch = build_fetch(&args_for(bin.path()));
	let conn = make_conn();
	let req = build_request("GET", "/cgi-bin/app", Body::Empty);
	let (status, headers, _body) = collect_body(invoke(&fetch, req, &conn).await).await;
	assert_eq!(status, http::StatusCode::FOUND);
	assert_eq!(headers.get("Location").and_then(|v| v.to_str().ok()), Some("/elsewhere"));
}

// ---------------------------------------------------------------------------
// 5. non-zero exit with no usable headers → 502
// ---------------------------------------------------------------------------

#[tokio::test]
async fn child_exits_non_zero_without_headers_yields_502() {
	let bin = tempbin(
		"#!/bin/sh\n\
		 echo 'no headers, just bail' >&2\n\
		 exit 7\n",
	);
	let fetch = build_fetch(&args_for(bin.path()));
	let conn = make_conn();
	let req = build_request("GET", "/cgi-bin/app", Body::Empty);
	let (status, _headers, _body) = collect_body(invoke(&fetch, req, &conn).await).await;
	assert_eq!(
		status,
		http::StatusCode::BAD_GATEWAY,
		"non-zero exit before producing a header block must surface 502",
	);
}

// ---------------------------------------------------------------------------
// 6. stderr → tracing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stderr_lines_emit_as_tracing_warn_under_vane_cgi_target() {
	use tracing_subscriber::layer::SubscriberExt as _;
	use tracing_subscriber::util::SubscriberInitExt as _;

	let layer = vane_engine::tracing_broadcast::BroadcastTracingLayer::new();
	let mut rx = layer.subscribe();
	let _guard = tracing_subscriber::registry().with(layer.clone()).set_default();

	// Three separate stderr lines so the captured stream has more than
	// one to match against.
	let bin = tempbin(
		"#!/bin/sh\n\
		 printf 'Status: 200 OK\\r\\n\\r\\n'\n\
		 echo 'cgi-stderr-line-one' >&2\n\
		 echo 'cgi-stderr-line-two' >&2\n\
		 printf 'ok'\n",
	);
	let fetch = build_fetch(&args_for(bin.path()));
	let conn = make_conn();
	let req = build_request("GET", "/cgi-bin/app", Body::Empty);
	let (status, _headers, _body) = collect_body(invoke(&fetch, req, &conn).await).await;
	assert_eq!(status, http::StatusCode::OK);

	// The stderr drain task spawns concurrently with stdout reading
	// + the child's own write-out + the final exit. Poll the
	// broadcast until both expected messages arrive or 3s elapses
	// (generous to avoid flake under full-workspace test pressure).
	let mut found: Vec<String> = Vec::new();
	let deadline = std::time::Instant::now() + Duration::from_secs(3);
	while std::time::Instant::now() < deadline
		&& !(found.iter().any(|s| s.contains("cgi-stderr-line-one"))
			&& found.iter().any(|s| s.contains("cgi-stderr-line-two")))
	{
		// `Ok(Ok(_))` is the only "real" arm — broadcast errors
		// (lagged / closed) and the outer timeout both just go round
		// the loop until the deadline; reading is "best effort".
		if let Ok(Ok(frame)) = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await
			&& frame.target == "vane::cgi"
			&& frame.level == "WARN"
		{
			// `message = %line` is captured into
			// `TracingFrame.message` (the special slot the visitor
			// splits out), not into the `fields` map.
			found.push(frame.message);
		}
	}
	assert!(
		found.iter().any(|s| s.contains("cgi-stderr-line-one")),
		"first stderr line must surface as a vane::cgi WARN: captured = {found:?}",
	);
	assert!(
		found.iter().any(|s| s.contains("cgi-stderr-line-two")),
		"second stderr line must surface as a vane::cgi WARN: captured = {found:?}",
	);
}

// ---------------------------------------------------------------------------
// 7. block_headers actually filters HTTP_*
// ---------------------------------------------------------------------------

#[tokio::test]
async fn block_headers_filters_authorization_from_http_env() {
	// Script lists the request's HTTP_* env vars (one per line). The
	// `block_headers` list contains "Authorization", so even though
	// the request carries `Authorization: Bearer secret`, the child's
	// env must *not* see HTTP_AUTHORIZATION.
	let bin = tempbin(
		"#!/bin/sh\n\
		 printf 'Status: 200 OK\\r\\n\\r\\n'\n\
		 env | grep '^HTTP_' | sort\n",
	);
	let fetch = build_fetch(&args_for(bin.path()));
	let conn = make_conn();
	let req = http::Request::builder()
		.method("GET")
		.uri("/cgi-bin/app")
		.header("host", "example.test")
		.header("authorization", "Bearer secret-token")
		.header("x-vane-allowed", "yes")
		.body(Body::Empty)
		.expect("build req");
	let (status, _headers, body) = collect_body(invoke(&fetch, req, &conn).await).await;
	assert_eq!(status, http::StatusCode::OK);
	let body = std::str::from_utf8(&body).expect("ascii env dump");
	assert!(
		!body.contains("HTTP_AUTHORIZATION"),
		"HTTP_AUTHORIZATION must be filtered by block_headers: {body}",
	);
	assert!(
		body.contains("HTTP_X_VANE_ALLOWED=yes"),
		"unblocked headers must still pass through: {body}",
	);
	assert!(body.contains("HTTP_HOST=example.test"), "Host header must pass through: {body}");
}

// ---------------------------------------------------------------------------
// 8. env with reserved key is a factory error (also covered in unit tests;
//    locked here at the integration layer to catch regressions in the
//    wiring between alias resolution and the factory dispatch.)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn env_with_reserved_request_method_is_factory_error() {
	let bin = tempbin("#!/bin/sh\necho ok\n");
	let mut args = args_for(bin.path());
	args["env"] = json!({ "REQUEST_METHOD": "FAKE" });
	match vane_engine::fetch::cgi::factory(&args) {
		Ok(_) => panic!("must reject reserved env key"),
		Err(e) => {
			assert!(e.0.contains("REQUEST_METHOD"), "must name the offending key: {e:?}");
		}
	}
}

// ---------------------------------------------------------------------------
// 9. binary that doesn't exist → factory error
// ---------------------------------------------------------------------------

#[tokio::test]
async fn missing_binary_is_factory_error() {
	let bin = tempbin("#!/bin/sh\necho ok\n");
	let mut args = args_for(bin.path());
	args["binary"] = json!("/no/such/cgi/binary-here");
	match vane_engine::fetch::cgi::factory(&args) {
		Ok(_) => panic!("must reject missing binary"),
		Err(e) => {
			assert!(e.0.contains("not accessible"), "must explain: {e:?}");
		}
	}
}

// ---------------------------------------------------------------------------
// 10. chroot reserved → factory error with spec wording
// ---------------------------------------------------------------------------

#[tokio::test]
async fn chroot_reserved_factory_error_matches_spec_wording() {
	let bin = tempbin("#!/bin/sh\necho ok\n");
	let mut args = args_for(bin.path());
	args["security"]["chroot"] = json!("/var/empty");
	match vane_engine::fetch::cgi::factory(&args) {
		Ok(_) => panic!("must reject chroot Some(_)"),
		Err(e) => {
			assert!(
				e.0.contains("chroot is reserved but not yet implemented"),
				"must use spec wording verbatim: {e:?}",
			);
		}
	}
}

// ---------------------------------------------------------------------------
// 12. total_timeout fires → 504
// ---------------------------------------------------------------------------

#[tokio::test]
async fn total_timeout_yields_504_and_kills_child() {
	// Script sleeps far longer than `total_timeout`. Without timeout
	// enforcement the test would hang for 10s; with enforcement it
	// returns 504 in ~200ms.
	let bin = tempbin(
		"#!/bin/sh\n\
		 sleep 10\n\
		 printf 'Status: 200 OK\\r\\n\\r\\nlate'\n",
	);
	let mut args = args_for(bin.path());
	args["timeouts"] = json!({ "total": "200ms", "connect": "200ms" });
	let fetch = build_fetch(&args);
	let conn = make_conn();
	let req = build_request("GET", "/cgi-bin/app", Body::Empty);

	// Wall-clock guard so a regression that disables the timeout
	// surfaces as a hang rather than an "ok in 10s" pass.
	let start = std::time::Instant::now();
	let resp = tokio::time::timeout(Duration::from_secs(5), invoke(&fetch, req, &conn))
		.await
		.expect("test must complete within 5s wall-clock");
	let elapsed = start.elapsed();
	let (status, _headers, _body) = collect_body(resp).await;
	assert_eq!(status, http::StatusCode::GATEWAY_TIMEOUT);
	assert!(elapsed < Duration::from_secs(3), "504 must surface promptly, took {elapsed:?}");
}

// ---------------------------------------------------------------------------
// Suppress unused-import warning on the `Mutex` in this file when
// every test path happens to reach the bare `parking_lot::Mutex` use
// inline; keep the helper import for readers who ext end the suite.
// ---------------------------------------------------------------------------
#[allow(dead_code)]
fn _force_mutex_use() -> Mutex<u8> {
	Mutex::new(0)
}
