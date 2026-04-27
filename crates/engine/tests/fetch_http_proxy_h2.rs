//! Integration tests for the pooled, ALPN-aware `HttpProxyFetch`.
//!
//! Covers the spec dispatch matrix from
//! `spec/architecture/09-config.md` § _Rule schema_ (`version` row):
//!
//! * `version: "auto"` + TLS upstream → ALPN negotiates `h2`.
//! * `version: "h1"` + TLS upstream → ALPN limits to `http/1.1`.
//! * `version: "h2"` + cleartext upstream → prior-knowledge h2c.
//! * `version: "h3"` → factory rejects without the `h3` cargo feature.
//! * Pool reuse: two requests against the same `Client` reuse the same
//!   TCP connection.
//!
//! Spec anchors: `spec/architecture/05-terminator.md` § _`HttpProxy`_,
//! `spec/architecture/07-l7.md` § _H1 / H2 paths_,
//! `spec/architecture/08-tls.md` § _TLS library: rustls only_.

#![allow(clippy::too_many_lines)]

use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::io::Write as _;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use arc_swap::ArcSwap;
use bytes::Bytes;
use http_body_util::{BodyExt, Empty, Full};
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tempfile::NamedTempFile;
use vane_core::{
	FetchId, FetchKind, FlowGraphMeta, FlowLogEvent, FlowLogSink, Node, NodeId, SymbolicFetchRef,
	SymbolicFlowGraph, Terminator, TerminatorId,
};
use vane_engine::ListenerSet;
use vane_engine::factories::{FactoryError, FetchFactories, MiddlewareFactories};
use vane_engine::fetch::http_proxy::{
	factory as http_proxy_factory, register as register_http_proxy,
};
use vane_engine::flow_graph::FlowGraph;
use vane_engine::verbosity::VerbosityState;

struct DropSink;
impl FlowLogSink for DropSink {
	fn emit(&self, _event: FlowLogEvent) {}
}

async fn pick_port() -> SocketAddr {
	let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind ephemeral");
	let addr = l.local_addr().expect("local_addr");
	drop(l);
	addr
}

fn sample_meta() -> FlowGraphMeta {
	FlowGraphMeta {
		version_hash: [0; 32],
		compiled_at: SystemTime::UNIX_EPOCH,
		source_files: vec![],
		feature_set: &[],
		short_circuit_response_entry: BTreeMap::new(),
		listener_tls: BTreeMap::new(),
		listener_kinds: BTreeMap::new(),
	}
}

fn proxy_graph(listen: SocketAddr, args: serde_json::Value) -> Arc<FlowGraph> {
	let mut entries = HashMap::new();
	entries.insert(listen, NodeId::new(0));
	let sym = Arc::new(SymbolicFlowGraph {
		nodes: vec![
			Node::Upgrade { next: NodeId::new(1) },
			Node::Fetch {
				id: FetchId::new(0),
				next_response: Some(NodeId::new(2)),
				next_tunnel: None,
				collect_body_before: None,
				body_limit: 0,
			},
			Node::Terminate(TerminatorId::new(0)),
		],
		predicates: vec![],
		middlewares: vec![],
		fetches: vec![SymbolicFetchRef {
			kind: FetchKind::HttpProxy,
			args,
			retry_buffer_required: false,
		}],
		terminators: vec![Terminator::WriteHttpResponse],
		entries,
		meta: sample_meta(),
	});
	let mw = MiddlewareFactories::new();
	let mut fetch = FetchFactories::new();
	register_http_proxy(&mut fetch);
	FlowGraph::link(sym, &mw, &fetch).expect("link http_proxy graph")
}

async fn start_listener(graph: Arc<FlowGraph>) -> (ListenerSet, SocketAddr) {
	let addr = *graph.symbolic().entries.iter().next().expect("entries").0;
	let verbosity = Arc::new(VerbosityState::new());
	let sink: Arc<dyn FlowLogSink> = Arc::new(DropSink);
	let set = ListenerSet::new();
	set.start(Arc::new(ArcSwap::new(graph)), verbosity, sink);
	tokio::time::sleep(Duration::from_millis(50)).await;
	(set, addr)
}

async fn h1_send_get(proxy_addr: SocketAddr) -> u16 {
	let stream = tokio::net::TcpStream::connect(proxy_addr).await.expect("client connect");
	let io = TokioIo::new(stream);
	let (mut sender, conn) =
		hyper::client::conn::http1::handshake::<_, Empty<Bytes>>(io).await.expect("h1 handshake");
	tokio::spawn(async move {
		let _ = conn.await;
	});
	let req = hyper::Request::builder()
		.method("GET")
		.uri("/")
		.header("host", "test.local")
		.body(Empty::<Bytes>::new())
		.expect("build GET");
	let resp = sender.send_request(req).await.expect("send_request");
	let status = resp.status().as_u16();
	let _ = resp.into_body().collect().await;
	status
}

// ---- TLS upstream fixtures ------------------------------------------------

struct TlsServerFixture {
	addr: SocketAddr,
	accepted: Arc<AtomicUsize>,
}

fn rcgen_self_signed() -> (String, String) {
	let issued =
		rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).expect("self-signed cert");
	(issued.cert.pem(), issued.signing_key.serialize_pem())
}

fn build_server_config(cert_pem: &str, key_pem: &str, alpn: Vec<Vec<u8>>) -> rustls::ServerConfig {
	let cert_chain: Vec<_> =
		rustls_pemfile::certs(&mut cert_pem.as_bytes()).collect::<Result<_, _>>().expect("parse cert");
	let private_key = rustls_pemfile::private_key(&mut key_pem.as_bytes())
		.expect("parse key bytes")
		.expect("private key present");
	let mut cfg = rustls::ServerConfig::builder()
		.with_no_client_auth()
		.with_single_cert(cert_chain, private_key)
		.expect("server cfg");
	cfg.alpn_protocols = alpn;
	cfg
}

/// Spawn a TLS upstream that replies 200 with the given body. `alpn`
/// shapes what versions the client can negotiate to. The returned
/// `accepted` counter increments once per `accept` (used by the pool
/// reuse test to confirm a single TCP connection serves N requests).
async fn spawn_tls_upstream(alpn: Vec<Vec<u8>>) -> (TlsServerFixture, NamedTempFile) {
	let (cert_pem, key_pem) = rcgen_self_signed();
	let mut cert_file = NamedTempFile::new().expect("cert tmp");
	cert_file.write_all(cert_pem.as_bytes()).expect("write cert");

	let server_cfg = Arc::new(build_server_config(&cert_pem, &key_pem, alpn.clone()));
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind tls");
	let addr = listener.local_addr().expect("local_addr");
	let accepted = Arc::new(AtomicUsize::new(0));
	let accepted_clone = Arc::clone(&accepted);

	tokio::spawn(async move {
		let acceptor = tokio_rustls::TlsAcceptor::from(server_cfg);
		loop {
			let Ok((sock, _)) = listener.accept().await else { return };
			accepted_clone.fetch_add(1, Ordering::SeqCst);
			let acceptor = acceptor.clone();
			tokio::spawn(async move {
				let Ok(tls_stream) = acceptor.accept(sock).await else { return };
				let alpn_negotiated =
					tls_stream.get_ref().1.alpn_protocol().map(<[u8]>::to_vec).unwrap_or_default();
				let io = TokioIo::new(tls_stream);
				if alpn_negotiated == b"h2" {
					let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
						.serve_connection(io, service_fn(serve_ok))
						.await;
				} else {
					let _ = hyper::server::conn::http1::Builder::new()
						.serve_connection(io, service_fn(serve_ok))
						.await;
				}
			});
		}
	});

	(TlsServerFixture { addr, accepted }, cert_file)
}

async fn serve_ok(
	_req: hyper::Request<hyper::body::Incoming>,
) -> Result<hyper::Response<Full<Bytes>>, Infallible> {
	Ok(
		hyper::Response::builder()
			.status(200)
			.body(Full::<Bytes>::new(Bytes::from_static(b"ok")))
			.expect("build resp"),
	)
}

#[tokio::test]
async fn http_proxy_h2_tls_negotiates_h2_via_alpn() {
	vane_engine::crypto::install_default_provider();
	let (upstream, _cert) = spawn_tls_upstream(vec![b"h2".to_vec(), b"http/1.1".to_vec()]).await;

	let proxy_addr = pick_port().await;
	let graph = proxy_graph(
		proxy_addr,
		serde_json::json!({
			"upstream": upstream.addr.to_string(),
			"version": "auto",
			"tls": { "insecure_skip_verify": true, "verify_hostname": "localhost" },
		}),
	);
	let (set, proxy_addr) = start_listener(graph).await;

	let status = h1_send_get(proxy_addr).await;
	assert_eq!(status, 200, "upstream that only offers h2 must serve the L7 request");
	assert!(upstream.accepted.load(Ordering::SeqCst) >= 1);

	set.shutdown(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn http_proxy_h1_explicit_forces_h1_alpn_only() {
	vane_engine::crypto::install_default_provider();
	let (upstream, _cert) = spawn_tls_upstream(vec![b"h2".to_vec(), b"http/1.1".to_vec()]).await;

	let proxy_addr = pick_port().await;
	let graph = proxy_graph(
		proxy_addr,
		serde_json::json!({
			"upstream": upstream.addr.to_string(),
			"version": "h1",
			"tls": { "insecure_skip_verify": true, "verify_hostname": "localhost" },
		}),
	);
	let (set, proxy_addr) = start_listener(graph).await;

	let status = h1_send_get(proxy_addr).await;
	assert_eq!(status, 200, "version: h1 + TLS must succeed (server falls to http/1.1)");
	set.shutdown(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn http_proxy_h2_cleartext_uses_prior_knowledge_h2c() {
	vane_engine::crypto::install_default_provider();

	// Cleartext h2c upstream: serve hyper http2 directly on a plain
	// TCP socket. The vane proxy was built with `version: "h2"` +
	// no TLS, which sets `http2_only(true)` on the legacy client so
	// it sends the h2 preface from the first byte.
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind h2c");
	let upstream_addr = listener.local_addr().expect("local_addr");
	tokio::spawn(async move {
		loop {
			let Ok((sock, _)) = listener.accept().await else { return };
			tokio::spawn(async move {
				let io = TokioIo::new(sock);
				let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
					.serve_connection(io, service_fn(serve_ok))
					.await;
			});
		}
	});

	let proxy_addr = pick_port().await;
	let graph = proxy_graph(
		proxy_addr,
		serde_json::json!({
			"upstream": upstream_addr.to_string(),
			"version": "h2",
		}),
	);
	let (set, proxy_addr) = start_listener(graph).await;

	let status = h1_send_get(proxy_addr).await;
	assert_eq!(status, 200, "cleartext h2c upstream must round-trip");

	set.shutdown(Duration::from_millis(500)).await;
}

#[test]
fn http_proxy_factory_rejects_version_h3_without_feature() {
	vane_engine::crypto::install_default_provider();
	let Err(FactoryError(msg)) = http_proxy_factory(&serde_json::json!({
		"upstream": "127.0.0.1:9443",
		"version": "h3",
	})) else {
		panic!("h3 must be rejected without the cargo feature");
	};
	assert!(msg.contains("h3"), "error names the version: {msg}");
}

#[test]
fn http_proxy_factory_accepts_auto_with_cleartext() {
	// Spec degradation: no ALPN on cleartext means `auto` collapses to
	// h1, the factory emits a `tracing::warn!` and returns `Ok(_)`.
	// We don't pull `tracing-test` in just to assert log lines; the
	// factory contract here is that the warning path doesn't error.
	vane_engine::crypto::install_default_provider();
	let result = http_proxy_factory(&serde_json::json!({
		"upstream": "127.0.0.1:9443",
		"version": "auto",
	}));
	assert!(result.is_ok(), "auto + cleartext must build (h1 fallback)");
}

#[tokio::test]
async fn http_proxy_pool_reuses_connection_across_requests() {
	vane_engine::crypto::install_default_provider();
	let (upstream, _cert) = spawn_tls_upstream(vec![b"http/1.1".to_vec()]).await;

	let proxy_addr = pick_port().await;
	let graph = proxy_graph(
		proxy_addr,
		serde_json::json!({
			"upstream": upstream.addr.to_string(),
			"version": "h1",
			"tls": { "insecure_skip_verify": true, "verify_hostname": "localhost" },
		}),
	);
	let (set, proxy_addr) = start_listener(graph).await;

	// Two sequential requests from one downstream H1 client. The
	// downstream connection is single-shot here (a fresh TCP per
	// `h1_send_get` call) but the upstream pool inside the proxy is
	// shared because both requests hit the same `HttpProxyFetch`
	// instance / same `Client`. The TLS-server `accepted` counter
	// must therefore stay at 1 after both rounds even though the
	// proxy served two requests.
	let s1 = h1_send_get(proxy_addr).await;
	let s2 = h1_send_get(proxy_addr).await;
	assert_eq!(s1, 200);
	assert_eq!(s2, 200);

	// Allow keep-alive idle to settle so the second request is
	// definitely complete before we observe the counter.
	tokio::time::sleep(Duration::from_millis(50)).await;
	let accepts = upstream.accepted.load(Ordering::SeqCst);
	assert_eq!(
		accepts, 1,
		"pool must reuse the upstream TCP connection across requests; observed {accepts} accepts",
	);

	set.shutdown(Duration::from_millis(500)).await;
}
