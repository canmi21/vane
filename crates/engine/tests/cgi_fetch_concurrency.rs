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

#![allow(unsafe_code)] // std::env::set_var is unsafe in 2024 edition; single-threaded binary isolates the race.

use std::io::Write as _;
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt as _;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::BodyExt as _;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

use tempfile::{NamedTempFile, TempDir};
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

/// A test fixture binary plus its enclosing tempdir. See the
/// matching helper in `cgi_fetch.rs` for why the write fd has to be
/// dropped before the path is handed to `execve`.
struct TempBin {
	_dir: TempDir,
	path: PathBuf,
}

impl TempBin {
	fn path(&self) -> &Path {
		&self.path
	}
}

fn tempbin(script: &str) -> TempBin {
	let dir = tempfile::tempdir().expect("tempdir");
	let path = dir.path().join("cgi_bin");
	{
		let mut f = std::fs::File::create(&path).expect("create");
		f.write_all(script.as_bytes()).expect("write");
	}
	std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
	TempBin { _dir: dir, path }
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
		// Generous wall-clock budgets — the slow-script branch sleeps
		// 1 s and CI runners under load can easily push fork+exec +
		// pre_exec drop + interpreter startup past a tighter
		// connect_timeout, even though the test itself doesn't care
		// about timing accuracy.
		"timeouts": { "connect": "30s", "total": "60s" },
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
	Arc::new(ConnContext::new(ConnId(1), remote, local, Transport::Tcp, std::time::Instant::now()))
}

fn make_ctx(conn: &Arc<ConnContext>) -> FlowCtx {
	FlowCtx {
		span: tracing::Span::none(),
		log: Arc::new(DropSink) as Arc<dyn FlowLogSink>,
		cancel: CancellationToken::new(),
		accept_cancel: CancellationToken::new(),
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

	// `pool_stats()` is `None` until the first CGI fetch runs (the
	// daemon-wide semaphore initialises lazily). Treat the
	// pre-fetch state as zero counters so the wait-for-permit loop
	// below has a stable baseline regardless of whether earlier
	// tests in this binary ever drove a fetch.
	let (allocations_baseline, failures_baseline) =
		vane_engine::fetch::cgi::pool_stats().map_or((0, 0), |s| (s.total_allocations, s.failures));

	let f1 = Arc::clone(&fetch);
	let c1 = make_conn();
	let first = tokio::spawn(async move { invoke_status(f1, c1).await });

	// Wait until the first request has actually acquired the only
	// permit before launching the second. Polling
	// `cgi::pool_stats().total_allocations` instead of sleeping a
	// fixed budget removes the timing race that surfaces under CI
	// load: a fixed 100 ms sleep used to intermittently let the
	// second request fire before the first had taken the permit,
	// flipping which request got the 503.
	let permit_deadline = std::time::Instant::now() + Duration::from_secs(5);
	loop {
		let cur = vane_engine::fetch::cgi::pool_stats().map_or(0, |s| s.total_allocations);
		if cur > allocations_baseline {
			break;
		}
		assert!(
			std::time::Instant::now() < permit_deadline,
			"first request never acquired the cgi permit within 5s",
		);
		tokio::time::sleep(Duration::from_millis(20)).await;
	}

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

	// Counters: one successful spawn (first), one cap-rejected
	// fast-reject (second). Compared against the baseline so the
	// test stays accurate when the binary's tests share process state.
	let stats = vane_engine::fetch::cgi::pool_stats().expect("pool initialised");
	assert!(
		stats.total_allocations > allocations_baseline,
		"first request bumped total_allocations: {} (baseline {})",
		stats.total_allocations,
		allocations_baseline,
	);
	assert!(
		stats.failures > failures_baseline,
		"second request bumped failures: {} (baseline {})",
		stats.failures,
		failures_baseline,
	);
}
