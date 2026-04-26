//! Integration tests for `vane_engine::fetch::websocket_upgrade`.
//!
//! Covers the H1→H1 WebSocket reverse-proxy contract described in
//! `spec/architecture/05-terminator.md` § _`WebSocketUpgrade`_:
//!
//! * On the upgrade path, vane forwards the client's HTTP/1.1
//!   `Upgrade: websocket` request to the upstream verbatim, awaits the
//!   upstream 101, captures the upgraded upstream IO, and writes the
//!   upstream 101 back to the client. After the 101 reaches the wire,
//!   `drive_h1_server`'s service-fn spawns a `copy_bidirectional` task
//!   that bridges client ↔ upstream. Bytes flow opaquely; vane never
//!   inspects WebSocket frames.
//! * Non-101 upstream responses (e.g. 403) are forwarded with the
//!   upstream body intact — no upgrade dance happens, no IO is stashed.
//! * Unreachable upstream surfaces as `Err(Error::upstream(...))`
//!   inside the L7 fetch; the H1 driver translates it into a
//!   synthetic 500.
//!
//! Each test wires a small TCP-level fake upstream + a vane
//! `ListenerSet` whose graph is
//! `Upgrade -> Fetch(WebSocketUpgrade{upstream}) ->
//! Terminate(WriteHttpResponse)`. The fake upstream is hand-rolled
//! against raw TCP rather than going through hyper because the tests
//! need full control over the wire bytes for the post-101 byte tunnel.

#![allow(clippy::too_many_lines)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use arc_swap::ArcSwap;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use vane_core::{
	FetchId, FetchKind, FlowGraphMeta, FlowLogEvent, FlowLogSink, Node, NodeId, SymbolicFetchRef,
	SymbolicFlowGraph, Terminator, TerminatorId,
};
use vane_engine::ListenerSet;
use vane_engine::factories::{FetchFactories, MiddlewareFactories};
use vane_engine::fetch::websocket_upgrade::register as register_ws;
use vane_engine::flow_graph::FlowGraph;
use vane_engine::verbosity::VerbosityState;

// ----- shared fixtures -----------------------------------------------------

struct DropSink;
impl FlowLogSink for DropSink {
	fn emit(&self, _event: FlowLogEvent) {}
}

async fn pick_port() -> SocketAddr {
	let l = TcpListener::bind("127.0.0.1:0").await.expect("bind");
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
		short_circuit_response_entry: std::collections::BTreeMap::new(),
		listener_tls: std::collections::BTreeMap::new(),
	}
}

/// Build a minimal L7 graph: `Upgrade -> Fetch(WebSocketUpgrade) ->
/// Terminate(WriteHttpResponse)`. The post-Upgrade entry is mapped in
/// `short_circuit_response_entry` so the executor can find a synth
/// target for any `Short(Response)` an L7 middleware emits — but the
/// WS fetch path never short-circuits, so the map content doesn't
/// affect the test's wire behavior.
fn ws_graph(listen: SocketAddr, upstream: &str) -> Arc<FlowGraph> {
	let mut entries = HashMap::new();
	entries.insert(listen, NodeId::new(0));

	let mut meta = sample_meta();
	// `lower_port` would normally populate this with the synth target;
	// we hand-build the graph so we hand-build the mapping. The map's
	// presence isn't strictly required for the WS happy path (the
	// fetch returns a 101 Response, not a Short), but populating it
	// keeps the graph self-consistent for the validator and any
	// future change that broadens the `Short(Response)` path.
	meta.short_circuit_response_entry.insert(NodeId::new(1), NodeId::new(2));

	let sym = Arc::new(SymbolicFlowGraph {
		nodes: vec![
			Node::Upgrade { next: NodeId::new(1) },
			Node::Fetch {
				id: FetchId::new(0),
				next_response: Some(NodeId::new(2)),
				next_tunnel: Some(NodeId::new(2)),
				collect_body_before: None,
			},
			Node::Terminate(TerminatorId::new(0)),
		],
		predicates: vec![],
		middlewares: vec![],
		fetches: vec![SymbolicFetchRef {
			kind: FetchKind::WebSocketUpgrade,
			args: serde_json::json!({ "upstream": upstream }),
		}],
		terminators: vec![Terminator::WriteHttpResponse],
		entries,
		meta,
	});
	let mw = MiddlewareFactories::new();
	let mut fetch = FetchFactories::new();
	register_ws(&mut fetch);
	FlowGraph::link(sym, &mw, &fetch).expect("link ws graph")
}

async fn start_listener(graph: Arc<FlowGraph>) -> (ListenerSet, SocketAddr) {
	let addr = *graph.symbolic().entries.iter().next().expect("entry present").0;
	let verbosity = Arc::new(VerbosityState::new());
	let sink: Arc<dyn FlowLogSink> = Arc::new(DropSink);
	let set = ListenerSet::new();
	set.start(Arc::new(ArcSwap::new(graph)), verbosity, sink);
	tokio::time::sleep(Duration::from_millis(50)).await;
	(set, addr)
}

/// Read bytes from `stream` until the buffered tail contains `\r\n\r\n`.
async fn read_until_headers_end(stream: &mut TcpStream) -> Vec<u8> {
	let mut buf = Vec::new();
	let mut tmp = [0u8; 1024];
	loop {
		let n = stream.read(&mut tmp).await.expect("read");
		if n == 0 {
			break;
		}
		buf.extend_from_slice(&tmp[..n]);
		if buf.windows(4).any(|w| w == b"\r\n\r\n") {
			break;
		}
	}
	buf
}

/// Spawn a fake WS upstream that:
/// 1. Reads the request headers until `\r\n\r\n`.
/// 2. Writes a fixed `HTTP/1.1 101 Switching Protocols` response.
/// 3. Echoes any subsequent bytes back to the client.
///
/// The accept loop handles a single connection then the task exits;
/// tests that need multiple connections call this multiple times.
async fn spawn_fake_ws_upstream_echo() -> SocketAddr {
	let listener = TcpListener::bind("127.0.0.1:0").await.expect("upstream bind");
	let addr = listener.local_addr().expect("addr");
	tokio::spawn(async move {
		let (mut sock, _peer) = listener.accept().await.expect("accept");
		let _ = read_until_headers_end(&mut sock).await;
		let resp = b"HTTP/1.1 101 Switching Protocols\r\n\
			Upgrade: websocket\r\n\
			Connection: Upgrade\r\n\
			Sec-WebSocket-Accept: RXEW6ax6BNRmDSUkBxiKlPFAoUM=\r\n\
			\r\n";
		sock.write_all(resp).await.expect("write 101");
		// Echo loop: read N bytes, write them back. Short reads are
		// fine — `copy_bidirectional` on the proxy side relays whatever
		// came through.
		let mut buf = [0u8; 4096];
		loop {
			let n = match sock.read(&mut buf).await {
				Ok(0) | Err(_) => break,
				Ok(n) => n,
			};
			if sock.write_all(&buf[..n]).await.is_err() {
				break;
			}
		}
	});
	addr
}

/// Spawn a fake upstream that returns an upgrade rejection (403) with
/// a body, then closes.
async fn spawn_fake_ws_upstream_reject() -> SocketAddr {
	let listener = TcpListener::bind("127.0.0.1:0").await.expect("upstream bind");
	let addr = listener.local_addr().expect("addr");
	tokio::spawn(async move {
		let (mut sock, _peer) = listener.accept().await.expect("accept");
		let _ = read_until_headers_end(&mut sock).await;
		let resp = b"HTTP/1.1 403 Forbidden\r\n\
			Content-Type: text/plain\r\n\
			Content-Length: 7\r\n\
			Connection: close\r\n\
			\r\n\
			no-auth";
		let _ = sock.write_all(resp).await;
	});
	addr
}

// ----- tests ---------------------------------------------------------------

#[tokio::test]
async fn ws_handshake_success_then_byte_tunnel_echoes() {
	let upstream = spawn_fake_ws_upstream_echo().await;
	let listen = pick_port().await;
	let graph = ws_graph(listen, &upstream.to_string());
	let (set, addr) = start_listener(graph).await;

	// Client: raw TCP. Send a minimal valid WS handshake; vane proxies
	// it to the upstream, which always replies 101 above. After the
	// 101, write 5 bytes; expect them echoed back.
	let mut client = TcpStream::connect(addr).await.expect("client connect");
	let req = b"GET / HTTP/1.1\r\n\
		Host: example\r\n\
		Upgrade: websocket\r\n\
		Connection: Upgrade\r\n\
		Sec-WebSocket-Key: dGVzdGtleQ==\r\n\
		Sec-WebSocket-Version: 13\r\n\
		\r\n";
	client.write_all(req).await.expect("client write req");

	// Read the 101 status line + headers.
	let buf = read_until_headers_end(&mut client).await;
	let head = std::str::from_utf8(&buf).expect("ascii head");
	assert!(head.starts_with("HTTP/1.1 101"), "expected 101, got: {head}");
	assert!(
		head.to_lowercase().contains("upgrade: websocket"),
		"upstream upgrade header should round-trip: {head}",
	);

	// Post-101: the vane service-fn spawned a `copy_bidirectional`.
	// Write a small payload; the upstream echoes it. Use a finite
	// retry on the read side because the bidi spawn races slightly
	// against the first `write_all` here.
	client.write_all(b"hello").await.expect("client write payload");
	let mut got = vec![0u8; 5];
	tokio::time::timeout(Duration::from_secs(2), client.read_exact(&mut got))
		.await
		.expect("echo read timeout")
		.expect("echo read");
	assert_eq!(&got, b"hello");

	set.shutdown(Duration::from_secs(2)).await;
}

#[tokio::test]
async fn ws_upstream_rejects_with_403_forwards_to_client() {
	let upstream = spawn_fake_ws_upstream_reject().await;
	let listen = pick_port().await;
	let graph = ws_graph(listen, &upstream.to_string());
	let (set, addr) = start_listener(graph).await;

	let mut client = TcpStream::connect(addr).await.expect("client connect");
	let req = b"GET / HTTP/1.1\r\n\
		Host: example\r\n\
		Upgrade: websocket\r\n\
		Connection: Upgrade\r\n\
		Sec-WebSocket-Key: dGVzdGtleQ==\r\n\
		Sec-WebSocket-Version: 13\r\n\
		\r\n";
	client.write_all(req).await.expect("client write req");

	let mut buf = Vec::new();
	tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut buf))
		.await
		.expect("read 403 timeout")
		.expect("read 403");
	let s = String::from_utf8_lossy(&buf);
	assert!(s.starts_with("HTTP/1.1 403"), "expected 403, got: {s}");
	assert!(s.contains("no-auth"), "upstream body should round-trip: {s}");

	set.shutdown(Duration::from_secs(2)).await;
}

#[tokio::test]
async fn ws_upstream_unreachable_surfaces_as_500() {
	// Pick a port + drop the listener so connect-refused is guaranteed.
	let upstream = pick_port().await;
	let listen = pick_port().await;
	let graph = ws_graph(listen, &upstream.to_string());
	let (set, addr) = start_listener(graph).await;

	let mut client = TcpStream::connect(addr).await.expect("client connect");
	let req = b"GET / HTTP/1.1\r\n\
		Host: example\r\n\
		Upgrade: websocket\r\n\
		Connection: Upgrade\r\n\
		Sec-WebSocket-Key: dGVzdGtleQ==\r\n\
		Sec-WebSocket-Version: 13\r\n\
		\r\n";
	client.write_all(req).await.expect("client write req");

	// Driver writes 500 + Content-Length: 0 without `Connection:
	// close`, so hyper keeps the keep-alive socket open. `read_to_end`
	// would hang; read just the status line + headers instead.
	let head = read_until_headers_end(&mut client).await;
	let s = String::from_utf8_lossy(&head);
	assert!(s.starts_with("HTTP/1.1 500"), "expected 500 from driver, got: {s}");

	set.shutdown(Duration::from_secs(2)).await;
}
