//! H3 dispatch DNS-resolution coverage. The factory takes
//! `args.dns` and feeds the resulting `HickoryDnsResolver` into the
//! H3 dial path; this test pins the resolver at an unreachable
//! nameserver and asserts the resulting dial fails with
//! `UpstreamReason::DnsFailure` rather than collapsing to the
//! system's `getaddrinfo` answer.

#![cfg(feature = "h3")]

use std::sync::Arc;

use parking_lot::Mutex;
use vane_core::{
	Body, ConnContext, ConnId, FlowCtx, FlowLogEvent, FlowLogSink, HttpVersion, NodeId, TlsInfo,
	TrajectoryBuilder, Transport, UpstreamReason,
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
		entered_at: std::time::Instant::now(),
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
		accept_cancel: tokio_util::sync::CancellationToken::new(),
		verbosity: VerbosityState::new().current(),
		trajectory: TrajectoryBuilder::new(conn.id, NodeId::new(0), 0),
	};
	(conn, ctx)
}

#[tokio::test(flavor = "multi_thread")]
async fn h3_dial_with_unreachable_custom_nameserver_surfaces_dns_failure() {
	vane_engine::crypto::install_default_provider();
	vane_testutil::allow_insecure_upstream_for_tests();

	// Custom nameserver pointing at a port where nothing listens.
	// hickory will time out / refuse on every query; the H3 dial
	// must surface `UpstreamReason::DnsFailure` rather than falling
	// back to the system resolver.
	let factory_args = serde_json::json!({
		"upstream": "does-not-exist.invalid:443",
		"version": "h3",
		"tls": { "insecure_skip_verify": true, "verify_hostname": "does-not-exist.invalid" },
		"dns": { "nameservers": ["127.0.0.1:1"] },
		// Short connect_timeout so the test cap stays under nextest's
		// slow-timeout. DNS failure should surface long before this.
		"connect_timeout": "5s",
	});
	let inst = vane_engine::fetch::http_proxy::factory(&factory_args, None).expect("factory");
	let FetchInst::L7(fetch) = inst else { panic!("expected L7") };

	let (conn, mut ctx) = make_ctx();
	let req = http::Request::builder().uri("http://placeholder/path").body(Body::Empty).expect("req");
	let Err(err) = fetch.fetch(req, &conn, &mut ctx).await else {
		panic!("dns failure expected, got Ok");
	};
	let formatted = format!("{err}");
	assert!(
		formatted.contains("dns resolution failed"),
		"expected dns resolution failed marker, got: {formatted}",
	);
}

#[tokio::test(flavor = "multi_thread")]
async fn h3_factory_accepts_ipv6_bracketed_upstream() {
	vane_engine::crypto::install_default_provider();
	vane_testutil::allow_insecure_upstream_for_tests();
	// Bracketed IPv6 literal upstream. The factory must split it into
	// host = `::1`, port = 8443 and accept; the dial path's resolver
	// short-circuit then converts the IP literal to `IpAddr::V6` with
	// no wire query.
	let factory_args = serde_json::json!({
		"upstream": "[::1]:8443",
		"version": "h3",
		"tls": { "insecure_skip_verify": true, "verify_hostname": "::1" },
	});
	let inst = vane_engine::fetch::http_proxy::factory(&factory_args, None);
	assert!(inst.is_ok(), "factory must accept bracketed IPv6 upstream: {:?}", inst.err());
	let _ = UpstreamReason::DnsFailure;
}
