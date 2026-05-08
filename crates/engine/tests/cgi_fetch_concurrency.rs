//! Concurrency-cap test for the CGI driver, isolated to its own
//! integration-test binary so the daemon-wide
//! `OnceLock<Semaphore>` it pokes doesn't bleed into other tests.
//!
//! `spec/crates/engine.md` § _Concurrency cap_: when the in-flight CGI count hits
//! `VANE_CGI_MAX_CONCURRENT`, new requests fast-reject with 503 — no
//! queueing. We set the cap to 1 via env, fire two requests against a
//! slow script, and assert the second one comes back as 503 promptly
//! while the first is still mid-fork.

// Tests in this file run sequentially within one binary; the only
// `unsafe` block sets `VANE_CGI_MAX_CONCURRENT` before any reader
// touches the OnceLock that initialises from it. There is no
// concurrent reader, so the standard `set_var` race condition that
// motivates the `unsafe` annotation in Rust 2024 doesn't apply here.
#![allow(unsafe_code)]
#![allow(clippy::too_many_lines)]

use std::io::Write as _;
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt as _;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::BodyExt as _;
use serde_json::{Value, json};
use tempfile::NamedTempFile;
use tokio_util::sync::CancellationToken;
use vane_core::{
	Body, ConnContext, ConnId, FlowCtx, FlowLogEvent, FlowLogSink, FlowLogVerbosity, L7Fetch,
	L7FetchOutput, TrajectoryBuilder, Transport,
};
use vane_engine::flow_graph::FetchInst;

struct DropSink;
impl FlowLogSink for DropSink {
	fn emit(&self, _event: FlowLogEvent) {}
}

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

fn args_for(bin: &std::path::Path) -> Value {
	json!({
		"upstream_kind": "cgi",
		"binary": bin.to_str().unwrap(),
		"script_name": "/cgi-bin/app",
		"working_dir": bin.parent().unwrap().to_str().unwrap(),
		"env": {},
		"block_headers": [],
		"security": {
			"uid": current_uid(),
			"gid": current_gid(),
			"limits": { "memory_mb": null, "cpu_seconds": null, "max_processes": null },
			"chroot": null,
		},
		"timeouts": { "connect": "5s", "total": "10s" },
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

async fn invoke_status(fetch: Arc<dyn L7Fetch>, conn: Arc<ConnContext>) -> http::StatusCode {
	let req = http::Request::builder()
		.method("GET")
		.uri("/cgi-bin/app")
		.header("host", "example.test")
		.body(Body::Empty)
		.expect("req");
	let mut ctx = make_ctx(&conn);
	let out = fetch.fetch(req, &conn, &mut ctx).await.expect("fetch");
	match out {
		L7FetchOutput::Response(r) => {
			let (parts, body) = r.into_parts();
			// Drain the body even for 503 / 504 to release the
			// owned permit (if any).
			let _ =
				body.collect().await.map_or_else(|_| Bytes::new(), http_body_util::Collected::to_bytes);
			parts.status
		}
		L7FetchOutput::Tunnel(_) => panic!("must be Response"),
	}
}

#[tokio::test]
async fn second_request_rejects_with_503_when_cap_is_one() {
	// Set the cap BEFORE the first CGI factory invocation in this
	// process so the OnceLock initializer sees it. Setting at the
	// start of the test (rather than via `cargo nextest --env`) is
	// safer because nextest's own environment isn't always pristine.
	// SAFETY: tests in this binary run sequentially; nothing else
	// reads VANE_CGI_MAX_CONCURRENT concurrently.
	unsafe {
		std::env::set_var("VANE_CGI_MAX_CONCURRENT", "1");
	}

	// Slow script — sleeps long enough for the second request to fire
	// while the first is still inside its permit window.
	let bin = tempbin(
		"#!/bin/sh\n\
		 sleep 1\n\
		 printf 'Status: 200 OK\\r\\n\\r\\nslow-ok'\n",
	);
	let fetch = build_fetch(&args_for(bin.path()));

	let f1 = Arc::clone(&fetch);
	let c1 = make_conn();
	let first = tokio::spawn(async move { invoke_status(f1, c1).await });

	// Brief delay so the first request acquires the permit before the
	// second probes it. 100ms is generous against the typical
	// fork+exec budget of a few ms on Linux/macOS.
	tokio::time::sleep(Duration::from_millis(100)).await;

	let f2 = Arc::clone(&fetch);
	let c2 = make_conn();
	let second = tokio::spawn(async move { invoke_status(f2, c2).await });

	let s2 = second.await.expect("second join");
	assert_eq!(
		s2,
		http::StatusCode::SERVICE_UNAVAILABLE,
		"second request must fast-reject with 503 while the first holds the only permit",
	);

	let s1 = first.await.expect("first join");
	assert_eq!(
		s1,
		http::StatusCode::OK,
		"first request must complete normally once the slow script returns"
	);

	// Counters: one successful spawn, one cap-rejected fast-reject.
	let stats = vane_engine::fetch::cgi::pool_stats().expect("pool initialised");
	assert!(
		stats.total_allocations >= 1,
		"first request bumped total_allocations: {}",
		stats.total_allocations,
	);
	assert!(stats.failures >= 1, "second request bumped failures: {}", stats.failures);
}
