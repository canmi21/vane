//! Retry coverage for the H3 dispatch arm of `HttpProxyFetch`.
//!
//! The TCP-family path already loops over retryable errors; the H3
//! arm now mirrors that shape via `dispatch_h3_with_retry`. End-to-
//! end validation against a real H3 mock server is heavyweight, so
//! these tests drive the dial path against a closed UDP port — the
//! handshake fails fast with `UpstreamReason::Unreachable` (which
//! `is_retryable() == true`), letting the retry loop run its full
//! shape. The timing-based attempt-count assertion checks that
//! backoff sleeps actually fire between attempts.

#![cfg(feature = "h3")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use vane_core::{
	Body, ConnContext, ConnId, FlowCtx, FlowLogEvent, FlowLogSink, HttpVersion, NodeId, TlsInfo,
	TrajectoryBuilder, Transport,
};
use vane_engine::flow_graph::FetchInst;
use vane_engine::verbosity::VerbosityState;

struct DropSink;
impl FlowLogSink for DropSink {
	fn emit(&self, _event: FlowLogEvent) {}
}

fn make_ctx() -> (Arc<ConnContext>, FlowCtx) {
	let conn = Arc::new(ConnContext {
		id: ConnId(1),
		remote: "127.0.0.1:0".parse().unwrap(),
		local: "127.0.0.1:0".parse().unwrap(),
		transport: Transport::Tcp,
		entered_at: Instant::now(),
		tls: Mutex::new(Some(TlsInfo {
			sni: None,
			alpn: None,
			version: None,
			peer_cert: None,
			zero_rtt_used: false,
		})),
		http_version: std::sync::OnceLock::from(HttpVersion::Http1_1),
		user: Mutex::new(http::Extensions::new()),
	});
	let span = tracing::info_span!("test");
	let ctx = FlowCtx {
		span,
		log: Arc::new(DropSink) as Arc<dyn FlowLogSink>,
		cancel: tokio_util::sync::CancellationToken::new(),
		verbosity: VerbosityState::new().current(),
		trajectory: TrajectoryBuilder::new(conn.id, NodeId::new(0), 0),
	};
	(conn, ctx)
}

/// Pick an ephemeral UDP port and immediately drop the socket, so
/// the address returned is highly likely to be unbound (and a quinn
/// dial there fails fast with ICMP unreachable / refused on
/// localhost). Using a fresh ephemeral port avoids the system's
/// reserved low ports (TCP port 1 vs UDP behavior varies).
async fn pick_unbound_udp_port() -> std::net::SocketAddr {
	let s = tokio::net::UdpSocket::bind("127.0.0.1:0").await.expect("bind ephemeral");
	let addr = s.local_addr().expect("local_addr");
	drop(s);
	addr
}

fn factory_args(
	addr: std::net::SocketAddr,
	max_attempts: u32,
	backoff_ms: u64,
) -> serde_json::Value {
	serde_json::json!({
		"upstream": addr.to_string(),
		"version": "h3",
		"tls": { "insecure_skip_verify": true, "verify_hostname": "localhost" },
		"connect_timeout": "200ms",
		"retry": {
			"max_attempts": max_attempts,
			"backoff": { "fixed": format!("{backoff_ms}ms") },
			// Force buffering so the request body always arrives as
			// `Body::Static` — but the retry tests below use Body::Empty
			// directly so this is belt-and-suspenders.
			"buffering": "opportunistic",
		},
	})
}

#[tokio::test(flavor = "multi_thread")]
async fn h3_retry_loops_on_unreachable_with_backoff() {
	vane_engine::crypto::install_default_provider();
	let addr = pick_unbound_udp_port().await;

	// max_attempts: 3, fixed 100ms backoff → at minimum 2 sleeps of
	// 100ms each between attempts. Total elapsed must be ≥ ~200ms.
	let inst =
		vane_engine::fetch::http_proxy::factory(&factory_args(addr, 3, 100), None).expect("factory");
	let FetchInst::L7(fetch) = inst else { panic!("L7") };

	let (conn, mut ctx) = make_ctx();
	let req = http::Request::builder().uri("http://placeholder/path").body(Body::Empty).expect("req");
	let start = Instant::now();
	let result = fetch.fetch(req, &conn, &mut ctx).await;
	let elapsed = start.elapsed();
	assert!(result.is_err(), "must fail against unreachable upstream");
	assert!(
		elapsed >= Duration::from_millis(180),
		"two backoff sleeps must elapse between three attempts; got {elapsed:?}",
	);
}

#[tokio::test(flavor = "multi_thread")]
async fn h3_no_retry_for_streaming_body_collapses_to_single_attempt() {
	vane_engine::crypto::install_default_provider();
	let addr = pick_unbound_udp_port().await;

	// `max_attempts: 5` would try five times if the body were
	// replayable; a `Body::Stream` collapses to a single attempt
	// regardless. Total elapsed must stay below the multi-attempt
	// threshold (no backoff fires).
	let inst =
		vane_engine::fetch::http_proxy::factory(&factory_args(addr, 5, 200), None).expect("factory");
	let FetchInst::L7(fetch) = inst else { panic!("L7") };

	let (conn, mut ctx) = make_ctx();
	let stream_body = Body::Stream(Box::pin(empty_stream_body()));
	let req = http::Request::builder().uri("http://placeholder/path").body(stream_body).expect("req");
	let start = Instant::now();
	let _ = fetch.fetch(req, &conn, &mut ctx).await;
	let elapsed = start.elapsed();
	// No backoff fired — must finish well before two 200ms sleeps
	// would have elapsed (400ms). Use 350ms with margin.
	assert!(
		elapsed < Duration::from_millis(350),
		"streaming body must collapse retry to single attempt; got {elapsed:?}",
	);
}

#[tokio::test(flavor = "multi_thread")]
async fn h3_no_retry_for_non_whitelisted_method_collapses_to_single_attempt() {
	vane_engine::crypto::install_default_provider();
	let addr = pick_unbound_udp_port().await;

	// Default whitelist is the RFC 7231 idempotent set
	// (GET / HEAD / PUT / DELETE / OPTIONS); POST is excluded.
	let inst =
		vane_engine::fetch::http_proxy::factory(&factory_args(addr, 5, 200), None).expect("factory");
	let FetchInst::L7(fetch) = inst else { panic!("L7") };

	let (conn, mut ctx) = make_ctx();
	let req = http::Request::builder()
		.method("POST")
		.uri("http://placeholder/path")
		.body(Body::Empty)
		.expect("req");
	let start = Instant::now();
	let _ = fetch.fetch(req, &conn, &mut ctx).await;
	let elapsed = start.elapsed();
	assert!(
		elapsed < Duration::from_millis(350),
		"POST is not in default whitelist → single attempt; got {elapsed:?}",
	);
}

#[tokio::test(flavor = "multi_thread")]
async fn h3_single_attempt_when_max_attempts_one() {
	vane_engine::crypto::install_default_provider();
	let addr = pick_unbound_udp_port().await;

	// max_attempts: 1 must short-circuit to send_one_attempt_h3
	// without entering the retry helper at all. Same elapsed
	// expectation as the streaming-body case.
	let inst =
		vane_engine::fetch::http_proxy::factory(&factory_args(addr, 1, 500), None).expect("factory");
	let FetchInst::L7(fetch) = inst else { panic!("L7") };

	let (conn, mut ctx) = make_ctx();
	let req = http::Request::builder().uri("http://placeholder/path").body(Body::Empty).expect("req");
	let start = Instant::now();
	let _ = fetch.fetch(req, &conn, &mut ctx).await;
	let elapsed = start.elapsed();
	assert!(
		elapsed < Duration::from_millis(450),
		"max_attempts=1 must skip backoff entirely; got {elapsed:?}",
	);
}

/// `Body::Stream` constructor for tests — yields no frames and
/// terminates immediately. Mirrors the empty-stream shape produced
/// by upstream adapters for zero-length streamed bodies.
fn empty_stream_body() -> impl http_body::Body<Data = bytes::Bytes, Error = vane_core::Error> + Send
{
	use std::pin::Pin;
	use std::task::{Context, Poll};

	struct Empty;
	impl http_body::Body for Empty {
		type Data = bytes::Bytes;
		type Error = vane_core::Error;
		fn poll_frame(
			self: Pin<&mut Self>,
			_cx: &mut Context<'_>,
		) -> Poll<Option<Result<http_body::Frame<bytes::Bytes>, Self::Error>>> {
			Poll::Ready(None)
		}
		fn is_end_stream(&self) -> bool {
			true
		}
	}
	Empty
}
