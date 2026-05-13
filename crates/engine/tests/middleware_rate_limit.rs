//! Integration tests for `vane_engine::middleware::rate_limit`.
//!
//! Validates the public contract from
//! `spec/crates/core.md` § _Rate limit (L2)_ and the doc-comment on
//! `rate_limit::factory`:
//!
//! - Token bucket math: tokens consumed on Continue, exhausted bucket
//!   yields `Decision::Short(ShortCircuit::Response(_))` with the
//!   configured (default 429) status.
//! - Per-key isolation: `key=remote_ip` keeps separate buckets per
//!   client IP; `key=global` shares one bucket across all clients.
//! - Factory validation: `window` range, missing `rate`/`burst`,
//!   `burst==0`, unsupported `key`, malformed `on_limit`.
//!
//! Treats the middleware as a black box — calls
//! `RateLimitMiddleware::run` directly via the registered factory.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Instant;

use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;
use vane_core::{
	ConnContext, ConnId, Decision, FlowCtx, FlowLogEvent, FlowLogSink, FlowLogVerbosity,
	ShortCircuit, TrajectoryBuilder, Transport,
};
use vane_engine::factories::MiddlewareFactories;
use vane_engine::flow_graph::MiddlewareInst;
use vane_engine::middleware::rate_limit;

// fixtures
struct DropSink;
impl FlowLogSink for DropSink {
	fn emit(&self, _event: FlowLogEvent) {}
}

fn make_conn(remote: &str) -> Arc<ConnContext> {
	let remote: SocketAddr = remote.parse().expect("remote");
	let local: SocketAddr = "127.0.0.1:0".parse().expect("local");
	Arc::new(ConnContext {
		id: ConnId(1),
		remote,
		local,
		transport: Transport::Tcp,
		entered_at: Instant::now(),
		tls: Mutex::new(None),
		http_version: OnceLock::new(),
		user: Mutex::new(http::Extensions::new()),
	})
}

fn make_ctx() -> FlowCtx {
	FlowCtx {
		span: tracing::Span::none(),
		log: Arc::new(DropSink) as Arc<dyn FlowLogSink>,
		cancel: CancellationToken::new(),
		accept_cancel: CancellationToken::new(),
		verbosity: FlowLogVerbosity::Trajectory,
		trajectory: TrajectoryBuilder::new(ConnId(1), vane_core::NodeId::new(0), 0),
	}
}

fn empty_request() -> http::Request<vane_core::Body> {
	http::Request::builder().method("GET").uri("/").body(vane_core::Body::Empty).expect("req")
}

/// Build a registered `RateLimitMiddleware` via the factory + return
/// the `L7Request` handle. The factory is the public entry the daemon
/// uses, so testing through it covers args validation + bucket
/// construction.
fn build_middleware(args: &serde_json::Value) -> Arc<dyn vane_core::L7RequestMiddleware> {
	let mut mw = MiddlewareFactories::new();
	rate_limit::register(&mut mw);
	let entry = mw.get("rate_limit").expect("rate_limit registered");
	match entry {
		vane_engine::factories::MiddlewareFactoryEntry::Available { construct, .. } => {
			let inst = construct(args).expect("factory accepts args");
			match inst {
				MiddlewareInst::L7Request(m) => m,
				other => panic!("expected L7Request, got {:?}", other.kind()),
			}
		}
		vane_engine::factories::MiddlewareFactoryEntry::FeatureGated(f) => {
			panic!("unexpected feature-gate {f}")
		}
	}
}

async fn try_call(mw: &dyn vane_core::L7RequestMiddleware, conn: &Arc<ConnContext>) -> Decision {
	let mut req = empty_request();
	let mut ctx = make_ctx();
	mw.run(&mut req, conn, &mut ctx).await.expect("middleware run")
}

fn factory_err(args: &serde_json::Value) -> String {
	let mut mw = MiddlewareFactories::new();
	rate_limit::register(&mut mw);
	let entry = mw.get("rate_limit").expect("rate_limit registered");
	let vane_engine::factories::MiddlewareFactoryEntry::Available { construct, .. } = entry else {
		panic!("expected available factory");
	};
	match construct(args) {
		Ok(_) => panic!("factory must reject these args"),
		Err(e) => e.0,
	}
}

// bucket math
#[tokio::test]
async fn rate_limit_allows_under_burst() {
	// burst=10, generous rate; 10 calls in a row all Continue.
	let mw = build_middleware(&serde_json::json!({ "rate": 100, "burst": 10, "window": "1s" }));
	let conn = make_conn("1.2.3.4:1000");
	for i in 0..10 {
		match try_call(&*mw, &conn).await {
			Decision::Continue => {}
			Decision::Short(_) => panic!("call #{i} unexpectedly short-circuited"),
			_ => panic!("call #{i} unexpected decision: <non-exhaustive Decision variant>"),
		}
	}
}

#[tokio::test]
async fn rate_limit_rejects_over_burst() {
	// burst=2; first two pass, third exhausts → Short(Response(429)).
	// Window is large enough that no refill happens between calls.
	let mw = build_middleware(&serde_json::json!({ "rate": 1, "burst": 2, "window": "60s" }));
	let conn = make_conn("1.2.3.4:1000");
	assert!(matches!(try_call(&*mw, &conn).await, Decision::Continue));
	assert!(matches!(try_call(&*mw, &conn).await, Decision::Continue));
	match try_call(&*mw, &conn).await {
		Decision::Short(ShortCircuit::Response(r)) => {
			assert_eq!(r.status().as_u16(), 429, "default rejection status");
		}
		_ => panic!("expected Short(Response(429))"),
	}
}

#[tokio::test]
async fn rate_limit_per_remote_ip_isolation() {
	// Two distinct IPs each have their own bucket. Exhausting one
	// must not affect the other.
	let mw = build_middleware(&serde_json::json!({ "rate": 1, "burst": 1, "window": "60s" }));
	let conn_a = make_conn("1.1.1.1:1000");
	let conn_b = make_conn("2.2.2.2:1000");

	// Exhaust A.
	assert!(matches!(try_call(&*mw, &conn_a).await, Decision::Continue));
	assert!(matches!(try_call(&*mw, &conn_a).await, Decision::Short(_)), "A exhausted");

	// B is unaffected.
	assert!(matches!(try_call(&*mw, &conn_b).await, Decision::Continue), "B has own bucket");
}

#[tokio::test]
async fn rate_limit_global_key_shared_across_ips() {
	// `key=global` collapses every client into one bucket; exhausting
	// from IP A blocks IP B.
	let mw = build_middleware(
		&serde_json::json!({ "key": "global", "rate": 1, "burst": 1, "window": "60s" }),
	);
	let conn_a = make_conn("1.1.1.1:1000");
	let conn_b = make_conn("2.2.2.2:1000");

	assert!(matches!(try_call(&*mw, &conn_a).await, Decision::Continue));
	match try_call(&*mw, &conn_b).await {
		Decision::Short(ShortCircuit::Response(_)) => {}
		_ => panic!("global bucket should reject B"),
	}
}

#[tokio::test]
async fn rate_limit_custom_on_limit_status_and_body() {
	use base64::Engine as _;
	let body_b64 = base64::engine::general_purpose::STANDARD.encode(b"teapot");
	let mw = build_middleware(&serde_json::json!({
		"rate": 1, "burst": 1, "window": "60s",
		"on_limit": {
			"status": 418,
			"headers": { "x-reason": "rate-limit" },
			"body": body_b64,
		}
	}));
	let conn = make_conn("9.9.9.9:9999");
	assert!(matches!(try_call(&*mw, &conn).await, Decision::Continue));
	match try_call(&*mw, &conn).await {
		Decision::Short(ShortCircuit::Response(r)) => {
			assert_eq!(r.status().as_u16(), 418);
			assert_eq!(r.headers().get("x-reason").and_then(|v| v.to_str().ok()), Some("rate-limit"));
			match r.body() {
				vane_core::Body::Static(b) => assert_eq!(&b[..], b"teapot"),
				_ => panic!("expected Body::Static(\"teapot\")"),
			}
		}
		_ => panic!("expected Short(Response(418))"),
	}
}

// factory validation
#[test]
fn rate_limit_factory_validates_window_range() {
	// Below 1s: rejected.
	let err = factory_err(&serde_json::json!({ "rate": 1, "burst": 1, "window": "0s" }));
	assert!(err.contains("[1s, 60s]"), "{err}");
	// Above 60s: rejected.
	let err = factory_err(&serde_json::json!({ "rate": 1, "burst": 1, "window": "61s" }));
	assert!(err.contains("[1s, 60s]"), "{err}");
	// Mid-range: accepted (no panic).
	let _mw = build_middleware(&serde_json::json!({ "rate": 1, "burst": 1, "window": "30s" }));
}

#[test]
fn rate_limit_factory_rejects_unsupported_key() {
	let err =
		factory_err(&serde_json::json!({ "key": "header", "rate": 1, "burst": 1, "window": "1s" }));
	assert!(err.contains("post-MVP"), "error mentions post-MVP: {err}");
	assert!(err.contains("\"header\""), "error quotes the offending key: {err}");
}

#[test]
fn rate_limit_factory_rejects_zero_burst() {
	let err = factory_err(&serde_json::json!({ "rate": 1, "burst": 0, "window": "1s" }));
	assert!(err.contains("≥ 1") || err.contains("burst"), "{err}");
}

#[test]
fn rate_limit_factory_rejects_missing_rate() {
	let err = factory_err(&serde_json::json!({ "burst": 1, "window": "1s" }));
	assert!(err.contains("rate"), "{err}");
}

#[test]
fn rate_limit_factory_rejects_invalid_window_format() {
	let err = factory_err(&serde_json::json!({ "rate": 1, "burst": 1, "window": "10" }));
	assert!(err.contains("'s'"), "error names the suffix: {err}");
}

#[test]
fn rate_limit_factory_rejects_on_limit_status_out_of_range() {
	let err = factory_err(&serde_json::json!({
		"rate": 1, "burst": 1, "window": "1s",
		"on_limit": { "status": 999 }
	}));
	assert!(err.contains("100-599"), "{err}");
}

// Suppress unused-var warning on `IpAddr` / `Ipv4Addr` if a future
// refactor drops them; keeps imports present alongside the SocketAddr
// fixtures.
const _: fn() -> Option<IpAddr> = || Some(IpAddr::V4(Ipv4Addr::LOCALHOST));
const _: fn() -> Option<HashMap<u8, u8>> = || None;
