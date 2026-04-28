//! UDP listener: physical socket bind + dispatch table + cold-path
//! `FlowGraph` entry. Hot-path datagrams are demultiplexed to the
//! registered [`DispatchHandle`] (currently only `L4Forward` sessions;
//! QUIC virtual sockets land later).
//!
//! See `spec/architecture/06-l4.md` § _`udp_dispatch`_ +
//! § _UDP socket multiplexing_.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use bytes::Bytes;
use dashmap::DashMap;
use tokio::net::UdpSocket;
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use vane_core::{
	ConnContext, FlowCtx, FlowLogSink, L4Conn, NodeId, TrajectoryBuilder, Transport, UdpAssoc,
};

use crate::executor::{ExecutorInput, execute};
use crate::flow_graph::FlowGraph;
use crate::verbosity::VerbosityState;

/// Maximum UDP datagram size. The recv buffer is sized for the IP
/// theoretical max (65535 minus IP+UDP headers, but we round up to
/// 65535 for simplicity — over-sized reads cost ~64 KiB per loop iter
/// which is negligible at expected per-listener traffic).
const MAX_DATAGRAM: usize = 65535;

/// Bounded inbound channel per `L4Forward` session. Full = drop, since
/// UDP is lossy by design and back-pressure to the listener loop would
/// stall every other session sharing the physical socket.
pub const SESSION_INBOUND_CAPACITY: usize = 64;

/// Demultiplex key for the per-listener dispatch table. Today only
/// 4-tuple peer identity is used (`L4Forward` sessions); the QUIC arm
/// is a placeholder so the type doesn't reshape when QUIC server
/// support arrives.
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub enum DispatchKey {
	Peer(SocketAddr),
	// FIXME(s3-01): `QuicConnId(quinn::ConnectionId)` — added when QUIC
	// server-side termination lands. Routed by ConnectionId so QUIC
	// peer migration across NAT rebinds keeps the same dispatch slot.
}

/// Demultiplex target for one dispatch table entry. Today only
/// `L4Forward(session)` is populated; the `Quic` arm awaits the QUIC
/// virtual-socket implementation.
pub enum DispatchHandle {
	L4Forward(Arc<L4ForwardSession>),
	// FIXME(s3-01): `Quic(Arc<VirtualUdpSocket>)` — once vane implements
	// `quinn::AsyncUdpSocket` against a per-connection wrapper.
}

/// Per-session forwarder handle. The listener pushes inbound datagrams
/// onto `inbound_tx` (drop-on-full); the session's spawned task pulls
/// them and forwards to upstream. `cancel` is fired by the executor's
/// `drive_byte_tunnel` arm when the per-connection cancel token (a
/// clone of the listener's `force_cancel`) trips.
pub struct L4ForwardSession {
	pub inbound_tx: mpsc::Sender<Bytes>,
	pub cancel: CancellationToken,
}

/// Listener-owned demultiplex table. Populated by `L4Forward` fetches
/// on cold-path entry (via the `Arc<DispatchTable>` stashed in
/// [`vane_core::ConnContext::user`]); cleared by the same fetch's
/// session-end cleanup future. Lives for the listener's lifetime.
pub type DispatchTable = DashMap<DispatchKey, Arc<DispatchHandle>>;

/// One running UDP listener task. Mirrors the TCP listener's handle
/// shape so [`crate::listener::ListenerSet`] can store both behind
/// one `HashMap<SocketAddr, _>`.
pub struct UdpListenerHandle {
	pub accept_cancel: CancellationToken,
	pub force_cancel: CancellationToken,
	pub in_flight: Arc<AsyncMutex<JoinSet<()>>>,
	pub in_flight_count: Arc<AtomicUsize>,
	pub bind_ready: Arc<AtomicBool>,
	pub join: tokio::task::JoinHandle<()>,
	pub dispatch_table: Arc<DispatchTable>,
}

/// Bind a UDP socket on `addr` with bind-retry, then run the recv +
/// dispatch loop until `accept_cancel` fires. Cold-path datagrams
/// spawn one tracked task each (`in_flight`), inheriting
/// `force_cancel` through `FlowCtx::cancel` for shutdown drain.
///
/// Spec: `06-l4.md` § _`udp_dispatch`_ for the dispatch table flow,
/// § _UDP idle timeout is single-authority_ for the per-session
/// timeout (owned by the `L4Forward` forwarder).
#[allow(clippy::too_many_arguments)]
pub async fn run_udp_listener(
	addr: SocketAddr,
	graph: Arc<ArcSwap<FlowGraph>>,
	verbosity: Arc<VerbosityState>,
	log_sink: Arc<dyn FlowLogSink>,
	accept_cancel: CancellationToken,
	force_cancel: CancellationToken,
	in_flight: Arc<AsyncMutex<JoinSet<()>>>,
	in_flight_count: Arc<AtomicUsize>,
	bind_ready: Arc<AtomicBool>,
	dispatch_table: Arc<DispatchTable>,
	max_bind_attempts: u32,
	bind_backoff_initial: Duration,
	bind_backoff_max: Duration,
) {
	let Some(socket) = bind_udp_with_retry(
		addr,
		&accept_cancel,
		max_bind_attempts,
		bind_backoff_initial,
		bind_backoff_max,
	)
	.await
	else {
		tracing::error!(
			?addr,
			attempts = max_bind_attempts,
			"udp listener bind failed after exhausting retries — giving up on this address",
		);
		return;
	};
	bind_ready.store(true, Ordering::Release);
	let socket = Arc::new(socket);

	let mut buf = vec![0u8; MAX_DATAGRAM];
	loop {
		tokio::select! {
			biased;
			() = accept_cancel.cancelled() => {
				// Recv loop exits; in-flight cold-path tasks will observe
				// `force_cancel` via their FlowCtx (drive_byte_tunnel arm
				// propagates into the forwarder's cancel token).
				return;
			}
			recv = socket.recv_from(&mut buf) => {
				let (n, peer) = match recv {
					Ok(v) => v,
					Err(e) => {
						tracing::debug!(?addr, ?e, "udp recv_from error; continuing");
						continue;
					}
				};
				let datagram = Bytes::copy_from_slice(&buf[..n]);
				let key = DispatchKey::Peer(peer);
				if let Some(entry) = dispatch_table.get(&key) {
					match &**entry {
						DispatchHandle::L4Forward(session) => {
							// Bounded channel; full = drop. UDP is lossy by
							// design and stalling the listener loop would
							// hold up every other session sharing the
							// physical socket.
							if session.inbound_tx.try_send(datagram).is_err() {
								tracing::warn!(
									target: "udp_forward",
									?peer,
									"udp session inbound channel full or closed; dropping datagram",
								);
							}
						}
					}
					continue;
				}
				// Cold path — enter the FlowGraph. Capture a graph
				// snapshot per-datagram so reload cannot pull the rug.
				let captured: Arc<FlowGraph> = graph.load_full();
				let Some(entry) = captured.symbolic().entries.get(&addr).copied() else {
					tracing::debug!(
						?addr,
						?peer,
						"udp cold path: no entry in active graph; dropping datagram",
					);
					continue;
				};
				in_flight_count.fetch_add(1, Ordering::Relaxed);
				let in_flight_guard = InFlightGuard(Arc::clone(&in_flight_count));
				in_flight.lock().await.spawn(handle_cold_path(
					Arc::clone(&socket),
					peer,
					datagram,
					addr,
					entry,
					captured,
					Arc::clone(&dispatch_table),
					Arc::clone(&verbosity),
					Arc::clone(&log_sink),
					force_cancel.clone(),
					in_flight_guard,
				));
			}
		}
	}
}

/// RAII decrement for `in_flight_count`. Mirrors `listener.rs::InFlightGuard`
/// so cold-path panics never leak the counter.
struct InFlightGuard(Arc<AtomicUsize>);

impl Drop for InFlightGuard {
	fn drop(&mut self) {
		self.0.fetch_sub(1, Ordering::Relaxed);
	}
}

/// Cold-path task: build the UDP `L4Conn`, stash the dispatch table
/// for the `L4Forward` fetch to register against, and walk the graph.
/// The fetch's spawned forwarder owns the session's lifetime; this
/// cold-path task lives as long as the executor is awaiting
/// `Tunnel::Udp::join`, which keeps `ConnContext` alive for the
/// session's duration.
#[allow(clippy::too_many_arguments)]
async fn handle_cold_path(
	socket: Arc<UdpSocket>,
	peer: SocketAddr,
	first_packet: Bytes,
	local: SocketAddr,
	entry: NodeId,
	graph: Arc<FlowGraph>,
	dispatch_table: Arc<DispatchTable>,
	verbosity: Arc<VerbosityState>,
	log_sink: Arc<dyn FlowLogSink>,
	force_cancel: CancellationToken,
	_in_flight_guard: InFlightGuard,
) {
	let conn_id = crate::listener::next_conn_id();
	let conn = Arc::new(ConnContext {
		id: conn_id,
		remote: peer,
		local,
		transport: Transport::Udp,
		entered_at: Instant::now(),
		tls: parking_lot::Mutex::new(None),
		http_version: std::sync::OnceLock::new(),
		user: parking_lot::Mutex::new(http::Extensions::new()),
	});
	// Stash the dispatch table so L4Forward (or any future UDP fetch)
	// can register a session under its own DispatchKey. Stored as
	// `Arc<DispatchTable>` so the fetch can cheaply clone it for
	// cleanup-on-shutdown.
	conn.user.lock().insert(Arc::clone(&dispatch_table));

	let span = tracing::info_span!("udp_conn", id = %conn.id);
	let mut ctx = FlowCtx {
		span,
		log: log_sink,
		cancel: force_cancel,
		verbosity: verbosity.current(),
		trajectory: TrajectoryBuilder::new(conn.id, entry, unix_ms_now()),
	};

	let l4 = L4Conn::Udp(UdpAssoc { socket, peer, first_packet, quic: None });
	let result = execute(&graph, entry, ExecutorInput::L4(Box::new(l4)), &conn, &mut ctx).await;
	if let Err(e) = result {
		tracing::warn!(error = %e, conn_id = %conn.id, "udp cold path ended with error");
	}
}

fn unix_ms_now() -> u64 {
	std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
		.unwrap_or_default()
}

async fn bind_udp_with_retry(
	addr: SocketAddr,
	cancel: &CancellationToken,
	max_attempts: u32,
	backoff_initial: Duration,
	backoff_max: Duration,
) -> Option<UdpSocket> {
	let mut delay = backoff_initial;
	for attempt in 0..max_attempts {
		if cancel.is_cancelled() {
			return None;
		}
		match UdpSocket::bind(addr).await {
			Ok(s) => return Some(s),
			Err(e) => {
				tracing::warn!(?addr, attempt, ?e, "udp bind failed");
			}
		}
		tokio::select! {
			biased;
			() = cancel.cancelled() => return None,
			() = tokio::time::sleep(delay) => {}
		}
		delay = (delay * 2).min(backoff_max);
	}
	None
}
