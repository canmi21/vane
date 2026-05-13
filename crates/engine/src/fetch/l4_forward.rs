//! `L4ForwardFetch` — TCP→TCP byte forwarding and UDP→UDP datagram
//! session forwarding.
//!
//! The TCP arm dials a fresh upstream per accepted connection and
//! returns a [`Tunnel::Bidi`] for the executor's
//! `Terminator::ByteTunnel` arm to drive via
//! `tokio::io::copy_bidirectional`. The UDP arm follows the cold/hot
//! path discipline of `spec/crates/engine.md` § _`udp_dispatch`_:
//! the listener delivers the first datagram via the cold path; the
//! fetch binds an ephemeral upstream socket, sends the first packet,
//! registers a session in the listener-owned dispatch table, and
//! spawns a 5-tuple forwarder task. Subsequent inbound datagrams from
//! the same peer hit the dispatch table and stream through the
//! forwarder without re-entering the `FlowGraph`.
//!
//! See `spec/crates/engine.md` `spec/crates/engine.md` § _Concrete fetches_ +
//! § _`udp_dispatch`_.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use vane_core::{
	AsyncReadWrite, CloseReason, ConnContext, Error, FetchKind, FlowCtx, L4Conn, L4Fetch, Transport,
	Tunnel, UdpTunnel, UpstreamReason,
};

use crate::factories::{FactoryError, FetchFactories};
use crate::fetch::retry::parse_duration;
use crate::flow_graph::FetchInst;
use crate::listener_udp::{
	DispatchHandle, DispatchKey, DispatchTable, L4ForwardSession, SESSION_INBOUND_CAPACITY,
};

const DEFAULT_UDP_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const UDP_RECV_BUFFER: usize = 65535;

/// Connects per-request (TCP) or per-5-tuple-session (UDP) to a
/// literal `host:port` upstream. The TCP arm hands the executor a
/// [`Tunnel::Bidi`]; the UDP arm hands a [`Tunnel::Udp`] whose `join`
/// future resolves when the spawned forwarder unwinds.
pub struct L4ForwardFetch {
	upstream: String,
	transport: Transport,
	idle_timeout: Duration,
}

#[async_trait]
impl L4Fetch for L4ForwardFetch {
	async fn fetch(
		&self,
		l4: L4Conn,
		conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<Tunnel, Error> {
		match (l4, self.transport) {
			(L4Conn::Udp(udp), Transport::Udp) => self.forward_udp(udp, conn).await,
			(L4Conn::Udp(_), Transport::Tcp) => Err(Error::internal(
				"L4Forward TCP-configured fetch received UDP packet — listener_transports \
				 derivation should have prevented this",
			)),
			(other, Transport::Udp) => {
				let _ = other;
				Err(Error::internal(
					"L4Forward UDP-configured fetch received non-UDP connection — listener_transports \
					 derivation should have prevented this",
				))
			}
			(L4Conn::Tcp(s), Transport::Tcp) => {
				let _ = s.set_nodelay(true);
				// Bare TCP both sides → emit `Tunnel::SpliceBidi` so
				// the executor can route through `splice(2)` on Linux
				// (and `copy_bidirectional` everywhere else); see
				// [`Self::forward_tcp_native`].
				self.forward_tcp_native(s).await
			}
			(L4Conn::Peeked(s), Transport::Tcp) => self.forward_tcp(s).await,
			(L4Conn::Tls(_), Transport::Tcp) => Err(Error::internal(
				"L4Forward fetch received a TLS-terminated stream — listener-tls + L4 byte forward is rejected by `lower_port`; this is a lower-stage invariant violation",
			)),
		}
	}
}

impl L4ForwardFetch {
	async fn forward_tcp(&self, client: Box<dyn AsyncReadWrite + Send>) -> Result<Tunnel, Error> {
		let upstream = TcpStream::connect(&self.upstream)
			.await
			.map_err(|e| Error::upstream(UpstreamReason::Unreachable).with_source(e))?;
		let _ = upstream.set_nodelay(true);
		Ok(Tunnel::Bidi {
			client,
			upstream: Box::new(upstream) as Box<dyn AsyncReadWrite + Send>,
			// L4 forward doesn't observe close reason; the executor's
			// `Terminator::ByteTunnel` arm sees `None` and skips the
			// oneshot send.
			close_reason_tx: None,
		})
	}

	/// TCP↔TCP fast path: both halves are bare `TcpStream`s so the
	/// executor can route them through `splice(2)` on Linux. This is
	/// the only arm that emits [`Tunnel::SpliceBidi`]; the `Peeked` /
	/// TLS / virtual arms fall through to [`Self::forward_tcp`] with
	/// a trait-object client.
	async fn forward_tcp_native(&self, client: TcpStream) -> Result<Tunnel, Error> {
		let upstream = TcpStream::connect(&self.upstream)
			.await
			.map_err(|e| Error::upstream(UpstreamReason::Unreachable).with_source(e))?;
		let _ = upstream.set_nodelay(true);
		Ok(Tunnel::SpliceBidi { client, upstream, close_reason_tx: None })
	}

	async fn forward_udp(
		&self,
		assoc: vane_core::UdpAssoc,
		conn: &Arc<ConnContext>,
	) -> Result<Tunnel, Error> {
		let upstream_addr: SocketAddr = self
			.upstream
			.parse()
			.map_err(|e| Error::upstream(UpstreamReason::Unreachable).with_source(e))?;
		// Bind an ephemeral source port, then connect so subsequent
		// `send` / `recv` calls implicit-default to the upstream addr.
		// The listener's physical socket stays in `assoc.socket`; the
		// forwarder owns this fresh upstream socket exclusively for the
		// 5-tuple session.
		let bind_local: SocketAddr =
			if upstream_addr.is_ipv6() { "[::]:0".parse() } else { "0.0.0.0:0".parse() }
				.expect("static bind addr parses");
		let start = std::time::Instant::now();
		let upstream_socket = UdpSocket::bind(bind_local)
			.await
			.map_err(|e| Error::upstream(UpstreamReason::Unreachable).with_source(e))?;
		upstream_socket
			.connect(upstream_addr)
			.await
			.map_err(|e| Error::upstream(UpstreamReason::Unreachable).with_source(e))?;
		metrics::histogram!("vane.upstream.connect.duration_ms", "kind" => "udp")
			.record(start.elapsed().as_secs_f64() * 1000.0);
		// Forward every cold-path datagram in arrival order so no inbound
		// bytes are lost between dispatch-table miss and forwarder
		// registration. Single-datagram is the common case; multi-datagram
		// arises from the pending-peek state machine (spec/crates/engine.md § _Multi-packet peek_).
		for pkt in &assoc.first_packets {
			upstream_socket
				.send(pkt)
				.await
				.map_err(|e| Error::upstream(UpstreamReason::Unreachable).with_source(e))?;
		}

		let dispatch_table =
			conn.user.lock().get::<Arc<DispatchTable>>().cloned().ok_or_else(|| {
				Error::internal(
					"L4Forward UDP path: dispatch table missing from ConnContext.user; \
					 listener_udp::handle_cold_path is responsible for stashing it",
				)
			})?;

		let cancel = CancellationToken::new();
		let (inbound_tx, inbound_rx) = mpsc::channel::<Bytes>(SESSION_INBOUND_CAPACITY);
		let session = Arc::new(L4ForwardSession { inbound_tx, cancel: cancel.clone() });
		let key = DispatchKey::Peer(assoc.peer);
		dispatch_table.insert(key.clone(), Arc::new(DispatchHandle::L4Forward(Arc::clone(&session))));

		let listener_socket = Arc::clone(&assoc.socket);
		let upstream_socket = Arc::new(upstream_socket);
		let peer = assoc.peer;
		let idle_timeout = self.idle_timeout;
		let cancel_for_task = cancel.clone();
		let upstream_for_task = Arc::clone(&upstream_socket);

		// RAII guard: removes the dispatch table entry on drop. Moved
		// into the spawned forwarder task so cleanup fires on every
		// exit path — natural completion, panic, or task abort during
		// shutdown. The outer `join` future used to do the remove
		// from the caller side, but it only ran if the caller drove
		// `join.await` to completion; aborting the executor would
		// leak the entry.
		let cleanup_guard =
			DispatchTableEntryGuard { table: Arc::clone(&dispatch_table), key: Some(key) };

		let join_handle = tokio::spawn(udp_forwarder_with_cleanup(
			cancel_for_task,
			inbound_rx,
			upstream_for_task,
			listener_socket,
			peer,
			idle_timeout,
			cleanup_guard,
		));

		// The outer `join` future surfaces the spawned task's
		// `CloseReason` to the executor. Cleanup is now inside the
		// task itself (via `DispatchTableEntryGuard::Drop`), so the
		// caller's `join.await` returning early is no longer a leak.
		let join: std::pin::Pin<Box<dyn std::future::Future<Output = CloseReason> + Send>> =
			Box::pin(async move {
				match join_handle.await {
					Ok(reason) => reason,
					Err(_join_err) => {
						CloseReason::ProtocolError(std::borrow::Cow::Borrowed("udp forwarder task panicked"))
					}
				}
			});
		Ok(Tunnel::Udp(UdpTunnel { join, cancel }))
	}
}

/// RAII cleanup for the per-session dispatch-table entry.
///
/// Owned by the spawned forwarder task so the dispatch table is
/// always cleaned up on task exit — natural completion, panic, or
/// abort. The guard's `Drop` removes `key` from the table; the
/// `Option<DispatchKey>` shape supports an explicit `take`-style
/// release path if we ever need one.
struct DispatchTableEntryGuard {
	table: Arc<DispatchTable>,
	key: Option<DispatchKey>,
}

impl Drop for DispatchTableEntryGuard {
	fn drop(&mut self) {
		if let Some(key) = self.key.take() {
			self.table.remove(&key);
		}
	}
}

/// Thin wrapper around [`udp_forwarder_loop`] that owns the
/// dispatch-table cleanup guard. Co-locating the guard with the
/// forwarder ensures cleanup fires even if the outer `join` future
/// is dropped before the forwarder exits.
async fn udp_forwarder_with_cleanup(
	cancel: CancellationToken,
	inbound_rx: mpsc::Receiver<Bytes>,
	upstream_socket: Arc<UdpSocket>,
	listener_socket: Arc<UdpSocket>,
	peer: SocketAddr,
	idle_timeout: Duration,
	_cleanup: DispatchTableEntryGuard,
) -> CloseReason {
	udp_forwarder_loop(cancel, inbound_rx, upstream_socket, listener_socket, peer, idle_timeout).await
}

/// Per-5-tuple forwarder loop. Owns one UDP socket connected to
/// upstream + a bounded inbound channel fed by the listener's recv
/// loop. The select arms are biased toward cancellation so a shutdown
/// signal wins races against in-flight datagrams.
///
/// `idle_timeout` is the single authority for session lifetime per
/// `spec/crates/engine.md` § _`udp_dispatch`_. The timer is reset on every datagram in either
/// direction; it fires only when the session has been quiet for the
/// configured duration.
async fn udp_forwarder_loop(
	cancel: CancellationToken,
	mut inbound_rx: mpsc::Receiver<Bytes>,
	upstream_socket: Arc<UdpSocket>,
	listener_socket: Arc<UdpSocket>,
	peer: SocketAddr,
	idle_timeout: Duration,
) -> CloseReason {
	let mut buf = vec![0u8; UDP_RECV_BUFFER];
	loop {
		// Allocate a fresh sleep on every iteration so the next
		// pair of datagrams resets the idle window. tokio::pin! moves
		// the timer into pin scope for the select.
		let timer = tokio::time::sleep(idle_timeout);
		tokio::pin!(timer);
		tokio::select! {
			biased;
			() = cancel.cancelled() => {
				return CloseReason::Cancelled;
			}
			() = &mut timer => {
				tracing::debug!(?peer, ?idle_timeout, "udp session idle timeout — closing");
				return CloseReason::Graceful;
			}
			maybe = inbound_rx.recv() => {
				let Some(bytes) = maybe else {
					// Channel closed — listener dropped the session
					// reference. Treat as graceful EOF.
					return CloseReason::Graceful;
				};
				if let Err(e) = upstream_socket.send(&bytes).await {
					tracing::debug!(?peer, ?e, "udp upstream send failed; closing session");
					return CloseReason::ProtocolError(std::borrow::Cow::Owned(format!(
						"udp upstream send: {e}"
					)));
				}
			}
			res = upstream_socket.recv(&mut buf) => {
				match res {
					Ok(n) => {
						if let Err(e) = listener_socket.send_to(&buf[..n], peer).await {
							tracing::debug!(?peer, ?e, "udp listener send_to failed; closing session");
							return CloseReason::ProtocolError(std::borrow::Cow::Owned(format!(
								"udp listener send_to: {e}"
							)));
						}
					}
					Err(e) => {
						tracing::debug!(?peer, ?e, "udp upstream recv failed; closing session");
						return CloseReason::ProtocolError(std::borrow::Cow::Owned(format!(
							"udp upstream recv: {e}"
						)));
					}
				}
			}
		}
	}
}

/// Args parser exposed as a registry-friendly factory. Args shape:
///
/// ```json
/// {
///   "upstream":     "host:port",
///   "transport":    "tcp" | "udp",
///   "idle_timeout": "30s"
/// }
/// ```
///
/// `transport` defaults to `"tcp"` and is normally injected by the
/// `tcp_forward` / `udp_forward` alias in
/// [`vane_core::rule::TerminateSpec`]. `idle_timeout` applies only to
/// the UDP arm and defaults to 30 s. Wider knobs (`tcp_keepalive`,
/// `dns_cache_ttl`) are post-MVP.
///
/// # Errors
/// Returns [`FactoryError`] when `upstream` is missing/empty, when
/// `transport` is not `"tcp"` / `"udp"`, or when `idle_timeout` is
/// not a parseable duration string.
pub fn factory(args: &serde_json::Value) -> Result<FetchInst, FactoryError> {
	let upstream = args.get("upstream").and_then(serde_json::Value::as_str).ok_or_else(|| {
		FactoryError::Invalid("missing args.upstream (string \"host:port\")".to_string())
	})?;
	if upstream.is_empty() {
		return Err(FactoryError::Invalid("args.upstream must not be empty".to_string()));
	}
	let transport_str = args.get("transport").and_then(serde_json::Value::as_str).unwrap_or("tcp");
	let transport = match transport_str {
		"tcp" => Transport::Tcp,
		"udp" => Transport::Udp,
		other => {
			return Err(FactoryError::Invalid(format!(
				"args.transport must be 'tcp' or 'udp', got {other:?}"
			)));
		}
	};
	let idle_timeout = match args.get("idle_timeout").and_then(serde_json::Value::as_str) {
		Some(s) => {
			parse_duration(s).map_err(|e| FactoryError::Invalid(format!("args.idle_timeout: {e}")))?
		}
		None => DEFAULT_UDP_IDLE_TIMEOUT,
	};
	Ok(FetchInst::L4(Arc::new(L4ForwardFetch {
		upstream: upstream.to_string(),
		transport,
		idle_timeout,
	})))
}

/// Convenience: register this fetch against `FetchKind::L4Forward` on a
/// `FetchFactories`.
pub fn register(factories: &mut FetchFactories) {
	factories.register(FetchKind::L4Forward, factory);
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	#[test]
	fn factory_defaults_to_tcp_transport() {
		let inst = factory(&json!({ "upstream": "127.0.0.1:9000" })).expect("ok");
		match inst {
			FetchInst::L4(_) => {}
			FetchInst::L7(_) => panic!("L4Forward must produce L4 inst"),
		}
	}

	#[test]
	fn factory_accepts_udp_transport() {
		let inst = factory(&json!({ "upstream": "1.2.3.4:53", "transport": "udp" })).expect("ok");
		assert!(matches!(inst, FetchInst::L4(_)));
	}

	#[test]
	fn factory_accepts_idle_timeout() {
		let inst = factory(&json!({
			"upstream": "1.2.3.4:53",
			"transport": "udp",
			"idle_timeout": "5s",
		}))
		.expect("ok");
		assert!(matches!(inst, FetchInst::L4(_)));
	}

	#[test]
	fn factory_rejects_unknown_transport() {
		let err = factory(&json!({ "upstream": "x:1", "transport": "sctp" })).err().expect("rejected");
		assert!(err.message().contains("'tcp' or 'udp'"), "{}", err.message());
	}

	#[test]
	fn factory_rejects_bad_idle_timeout() {
		let err = factory(&json!({
			"upstream": "x:1",
			"transport": "udp",
			"idle_timeout": "forever",
		}))
		.err()
		.expect("rejected");
		assert!(err.message().contains("idle_timeout"), "{}", err.message());
	}

	#[test]
	fn factory_rejects_missing_upstream() {
		match factory(&json!({})) {
			Ok(_) => panic!("must reject missing upstream"),
			Err(e) => assert!(e.message().contains("upstream"), "{}", e.message()),
		}
	}

	#[test]
	fn factory_rejects_empty_upstream() {
		match factory(&json!({ "upstream": "" })) {
			Ok(_) => panic!("must reject empty upstream"),
			Err(e) => assert!(e.message().contains("must not be empty"), "{}", e.message()),
		}
	}
}
