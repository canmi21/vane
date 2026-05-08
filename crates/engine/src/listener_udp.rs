//! UDP listener: physical socket bind + dispatch table + cold-path
//! `FlowGraph` entry. Hot-path datagrams are demultiplexed to the
//! registered [`DispatchHandle`]: live `L4Forward` sessions, in-
//! formation pending-peek sessions (QUIC SNI passthrough — see
//! `spec/crates/engine.md` § _Multi-packet peek_), and the per-listener QUIC
//! virtual socket on `Http` UDP listeners.
//!
//! See `spec/crates/engine.md` § _`udp_dispatch`_ +
//! § _`udp_dispatch`_.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use bytes::Bytes;
use clienthello::{Extractor, PushOutcome};
use dashmap::DashMap;
use parking_lot::Mutex;
use tokio::net::UdpSocket;
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use vane_core::{
	ConnContext, FlowCtx, FlowLogSink, L4Conn, NodeId, TlsInfo, TrajectoryBuilder, Transport,
	UdpAssoc,
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

/// Per `spec/crates/engine.md` § _Multi-packet peek_ § _Multi-packet peek_.
/// Values are fixed (not configurable) — the spec table justifies each.
pub const PENDING_PEEK_MAX_BYTES: usize = 16 * 1024;
pub const PENDING_PEEK_MAX_DATAGRAMS: usize = 8;
pub const PENDING_PEEK_LIFETIME: Duration = Duration::from_secs(1);
pub const PENDING_PEEK_MAX_PER_LISTENER: usize = 1024;

/// Demultiplex key for the per-listener dispatch table.
///
/// `Peer` keys 4-tuple-identified `L4Forward` sessions. `PendingPeek`
/// keys cold-path sessions in formation (the QUIC SNI passthrough
/// state machine). `QuicConnId` keys the per-listener QUIC virtual
/// socket — `spec/crates/engine.md` § _UDP socket multiplexing: physical and
/// virtual_ locks one `quinn::Endpoint` per `Http` UDP listener, so
/// that variant only ever holds one entry per listener at the
/// empty-CID slot. `vane` does not index by per-connection CID;
/// `quinn::Endpoint` performs CID-keyed demultiplexing internally for
/// the connections it terminates. The `QuicConnId` variant is
/// `#[cfg(feature = "h3")]`-gated so non-H3 builds don't pull
/// `quinn-proto` into the type.
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub enum DispatchKey {
	Peer(SocketAddr),
	/// Cold-path session in formation, keyed by peer 4-tuple. Per
	/// `spec/crates/engine.md` § _Multi-packet peek_ the SCID is intentionally not used
	/// (the first datagram has not been parsed at lookup time, so the
	/// SCID is unavailable).
	PendingPeek(SocketAddr),
	#[cfg(feature = "h3")]
	QuicConnId(quinn_proto::ConnectionId),
}

/// Demultiplex target for one dispatch table entry. `L4Forward` carries
/// the per-session forwarder handle; `PendingPeek` carries the
/// in-formation cold-path state; `Quic` carries the per-listener
/// virtual UDP socket that `quinn::Endpoint` drives. Inbound datagrams
/// are routed to one or the other (or fall through to the cold path
/// on miss).
pub enum DispatchHandle {
	L4Forward(Arc<L4ForwardSession>),
	PendingPeek(Arc<PendingPeekState>),
	#[cfg(feature = "h3")]
	Quic(Arc<crate::h3::listener::VirtualUdpSocket>),
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

/// In-formation cold-path session: an `Extractor` accumulating QUIC
/// Initial datagrams in arrival order until the SNI is parsed (or a
/// bound is exceeded). Per-session bounds are enforced on every push
/// against the spec values in `PENDING_PEEK_MAX_*` / `PENDING_PEEK_LIFETIME`.
pub struct PendingPeekState {
	extractor: Mutex<Extractor>,
	/// Buffered raw datagrams; replayed in arrival order to the matched
	/// `L4Forward` handler when SNI extraction completes.
	datagrams: Mutex<Vec<Bytes>>,
	/// Total bytes accumulated across `datagrams`. Tracked separately
	/// so the bound check is cheap.
	bytes: AtomicUsize,
	started_at: Instant,
}

impl PendingPeekState {
	fn new() -> Self {
		Self {
			extractor: Mutex::new(Extractor::new()),
			datagrams: Mutex::new(Vec::new()),
			bytes: AtomicUsize::new(0),
			started_at: Instant::now(),
		}
	}
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
/// Spec: `spec/crates/engine.md` § _`udp_dispatch`_ for the dispatch table flow,
/// § _`udp_dispatch`_ for the per-session
/// timeout (owned by the `L4Forward` forwarder).
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
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

	// On UDP+Http listeners, bring up the H3 stack: per-listener
	// quinn::Endpoint wrapping a VirtualUdpSocket, registered in the
	// dispatch table under the well-known `QuicConnId(empty)` slot —
	// see `spec/crates/engine.md` § _`udp_dispatch`_.
	#[cfg(feature = "h3")]
	{
		let captured = graph.load_full();
		let kind = captured
			.symbolic()
			.meta
			.listener_kinds
			.get(&addr)
			.copied()
			.unwrap_or(vane_core::ListenerKind::Raw);
		if matches!(kind, vane_core::ListenerKind::Http) {
			if let Some(tls_cfg) = captured.listener_tls(&addr).cloned() {
				match crate::h3::listener::spawn_h3_endpoint(
					addr,
					Arc::clone(&socket),
					tls_cfg,
					Arc::clone(&dispatch_table),
					Arc::clone(&graph),
					Arc::clone(&log_sink),
					Arc::clone(&verbosity),
					force_cancel.clone(),
				) {
					Ok(()) => {
						tracing::info!(?addr, "h3 listener up");
					}
					Err(e) => {
						tracing::error!(?addr, error = %e, "h3 endpoint setup failed");
					}
				}
			} else {
				tracing::warn!(
					?addr,
					"udp Http listener has no listener_tls config; H3 requires TLS — skipping H3 setup",
				);
			}
		}
		drop(captured);
	}

	// Concurrent pending-peek sessions on this listener. Incremented
	// when a PendingPeek entry is inserted, decremented on removal.
	// Used to enforce the `PENDING_PEEK_MAX_PER_LISTENER` cap without
	// scanning the whole dispatch table per cold-path datagram.
	let pending_count: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(0));

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
						DispatchHandle::PendingPeek(_) => {
							// `Peer(peer)` cannot legally point at a
							// PendingPeek handle — pending-peek lives under
							// the `PendingPeek(peer)` key. This branch is
							// unreachable in correct code; trace and drop.
							tracing::warn!(
								?peer,
								"dispatch table internal invariant: Peer(_) → PendingPeek; dropping",
							);
						}
						#[cfg(feature = "h3")]
						DispatchHandle::Quic(virtual_socket) => {
							virtual_socket.enqueue_inbound(peer, datagram);
						}
					}
					continue;
				}

				// Pending-peek state machine, per `spec/crates/engine.md` §
				// _Multi-packet peek_ § _Multi-packet peek_. Existing
				// PendingPeek entries always get the datagram first,
				// regardless of listener kind.
				let pending_key = DispatchKey::PendingPeek(peer);
				if let Some(entry) = dispatch_table.get(&pending_key)
					&& let DispatchHandle::PendingPeek(state) = &**entry {
						let state = Arc::clone(state);
						drop(entry);
						match advance_pending_peek(&state, &datagram) {
							PendingAdvance::Sni(sni) => {
								let datagrams = drain_pending(&state, datagram);
								if dispatch_table.remove(&pending_key).is_some() {
									pending_count.fetch_sub(1, Ordering::Relaxed);
								}
								spawn_cold_path(
									&socket,
									&dispatch_table,
									&graph,
									&verbosity,
									&log_sink,
									&force_cancel,
									&in_flight,
									&in_flight_count,
									addr,
									peer,
									datagrams,
									Some(sni),
								)
								.await;
							}
							PendingAdvance::NeedMore => {
								// Buffered for later datagram; no spawn.
							}
							PendingAdvance::Drop => {
								if dispatch_table.remove(&pending_key).is_some() {
									pending_count.fetch_sub(1, Ordering::Relaxed);
								}
							}
						}
						continue;
					}

				// On Http UDP listeners, route any unmatched datagram to
				// the listener-level QUIC virtual socket — one per
				// listener per `spec/crates/engine.md` § _UDP socket multiplexing:
				// physical and virtual_; `quinn::Endpoint` then performs
				// CID-keyed demultiplexing internally for the connections
				// it terminates.
				#[cfg(feature = "h3")]
				let datagram = match try_route_to_h3(&graph, &dispatch_table, addr, peer, datagram) {
					RouteH3::Routed => continue,
					RouteH3::NotApplicable(d) => d,
				};

				// Cold path — capture a graph snapshot per-datagram so
				// reload cannot pull the rug. Decide whether to enter
				// pending-peek (multi-packet QUIC SNI extraction) or go
				// straight to the FlowGraph entry.
				let captured: Arc<FlowGraph> = graph.load_full();
				let Some(entry) = captured.symbolic().entries.get(&addr).copied() else {
					tracing::debug!(
						?addr,
						?peer,
						"udp cold path: no entry in active graph; dropping datagram",
					);
					continue;
				};

				if captured.needs_pending_peek(addr, entry)
					&& is_quic_long_header_initial(&datagram)
				{
					if pending_count.load(Ordering::Relaxed) >= PENDING_PEEK_MAX_PER_LISTENER {
						// `spec/crates/engine.md` § _Multi-packet peek_: silent drop past the
						// per-listener cap. Operators see the drop
						// only via metrics / counts — no per-drop log
						// to avoid amplifying a flood.
						continue;
					}
					let state = Arc::new(PendingPeekState::new());
					match advance_pending_peek(&state, &datagram) {
						PendingAdvance::Sni(sni) => {
							let datagrams = drain_pending(&state, datagram);
							spawn_cold_path(
								&socket,
								&dispatch_table,
								&graph,
								&verbosity,
								&log_sink,
								&force_cancel,
								&in_flight,
								&in_flight_count,
								addr,
								peer,
								datagrams,
								Some(sni),
							)
							.await;
						}
						PendingAdvance::NeedMore => {
							let handle = Arc::new(DispatchHandle::PendingPeek(Arc::clone(&state)));
							if dispatch_table.insert(pending_key, handle).is_none() {
								pending_count.fetch_add(1, Ordering::Relaxed);
							}
						}
						PendingAdvance::Drop => {
							// First datagram failed parsing — fall back
							// to immediate cold-path entry with the
							// raw datagram, mirroring spec behavior of
							// "not Initial → fall through".
							spawn_cold_path(
								&socket,
								&dispatch_table,
								&graph,
								&verbosity,
								&log_sink,
								&force_cancel,
								&in_flight,
								&in_flight_count,
								addr,
								peer,
								vec![datagram],
								None,
							)
							.await;
						}
					}
					continue;
				}

				spawn_cold_path(
					&socket,
					&dispatch_table,
					&graph,
					&verbosity,
					&log_sink,
					&force_cancel,
					&in_flight,
					&in_flight_count,
					addr,
					peer,
					vec![datagram],
					None,
				)
				.await;
			}
		}
	}
}

/// Outcome of a single [`advance_pending_peek`] step.
enum PendingAdvance {
	Sni(String),
	NeedMore,
	Drop,
}

/// Push one datagram into the `PendingPeekState` and apply the spec's
/// bound checks. Bounds: bytes ≤ 16 KiB, datagram count ≤ 8, lifetime
/// ≤ 1 s. Exceeding any bound, or an extraction error, returns
/// [`PendingAdvance::Drop`] so the caller removes the entry.
fn advance_pending_peek(state: &PendingPeekState, datagram: &Bytes) -> PendingAdvance {
	if state.started_at.elapsed() > PENDING_PEEK_LIFETIME {
		return PendingAdvance::Drop;
	}
	let new_bytes = state.bytes.load(Ordering::Relaxed).saturating_add(datagram.len());
	if new_bytes > PENDING_PEEK_MAX_BYTES {
		return PendingAdvance::Drop;
	}
	{
		let mut buf = state.datagrams.lock();
		if buf.len() >= PENDING_PEEK_MAX_DATAGRAMS {
			return PendingAdvance::Drop;
		}
		buf.push(datagram.clone());
	}
	state.bytes.store(new_bytes, Ordering::Relaxed);

	match state.extractor.lock().push(datagram) {
		Ok(PushOutcome::Sni(s)) => PendingAdvance::Sni(s),
		Ok(PushOutcome::NeedMore) => PendingAdvance::NeedMore,
		Err(_) => PendingAdvance::Drop,
	}
}

/// Drain the buffered datagrams out of a pending-peek state so they
/// can be replayed to the matched `L4Forward` handler. The triggering
/// datagram is already buffered (added by `advance_pending_peek` for
/// this push) — the caller passes it for a sanity assertion only.
fn drain_pending(state: &PendingPeekState, _completing_datagram: Bytes) -> Vec<Bytes> {
	let mut buf = state.datagrams.lock();
	std::mem::take(&mut *buf)
}

/// Recognize a QUIC long-header packet whose type is Initial. Used by
/// the cold-path miss to gate the pending-peek state machine: only
/// QUIC Initial datagrams enter pending-peek; non-QUIC datagrams (DNS,
/// non-QUIC `L4Forward` traffic) take the immediate cold-path entry.
///
/// Per RFC 9000 §17.2, the first byte of a long-header Initial packet
/// has bits: form=1, fixed=1, type=00, reserved (header-protected) +
/// PN length (header-protected). The first nibble must equal `0xc0`.
fn is_quic_long_header_initial(datagram: &[u8]) -> bool {
	datagram.first().is_some_and(|first| first & 0xf0 == 0xc0)
}

#[allow(clippy::too_many_arguments)]
async fn spawn_cold_path(
	socket: &Arc<UdpSocket>,
	dispatch_table: &Arc<DispatchTable>,
	graph: &Arc<ArcSwap<FlowGraph>>,
	verbosity: &Arc<VerbosityState>,
	log_sink: &Arc<dyn FlowLogSink>,
	force_cancel: &CancellationToken,
	in_flight: &Arc<AsyncMutex<JoinSet<()>>>,
	in_flight_count: &Arc<AtomicUsize>,
	addr: SocketAddr,
	peer: SocketAddr,
	first_packets: Vec<Bytes>,
	sni: Option<String>,
) {
	let captured: Arc<FlowGraph> = graph.load_full();
	let Some(entry) = captured.symbolic().entries.get(&addr).copied() else {
		tracing::debug!(
			?addr,
			?peer,
			"udp cold path: no entry in active graph at spawn time; dropping datagram",
		);
		return;
	};
	in_flight_count.fetch_add(1, Ordering::Relaxed);
	let in_flight_guard = InFlightGuard(Arc::clone(in_flight_count));
	in_flight.lock().await.spawn(handle_cold_path(
		Arc::clone(socket),
		peer,
		first_packets,
		sni,
		addr,
		entry,
		captured,
		Arc::clone(dispatch_table),
		Arc::clone(verbosity),
		Arc::clone(log_sink),
		force_cancel.clone(),
		in_flight_guard,
	));
}

/// Outcome of [`try_route_to_h3`]: either the datagram was delivered
/// to the H3 fan-in socket (and the caller's loop must `continue`), or
/// the listener is not configured for H3 and the datagram is returned
/// unchanged for cold-path entry.
#[cfg(feature = "h3")]
enum RouteH3 {
	Routed,
	NotApplicable(Bytes),
}

/// Listener-level fan-in: deliver an unmatched UDP datagram to the
/// per-listener QUIC virtual socket if and only if the listener's
/// derived [`vane_core::ListenerKind`] is `Http`. The virtual socket
/// is registered at listener boot under the well-known
/// `QuicConnId(empty)` slot — `spec/crates/engine.md` § _UDP socket multiplexing:
/// physical and virtual_ holds one virtual socket per listener, so
/// the empty-CID slot is the listener's single QUIC fan-in entry
/// rather than a per-connection key.
#[cfg(feature = "h3")]
fn try_route_to_h3(
	graph: &Arc<ArcSwap<FlowGraph>>,
	dispatch_table: &Arc<DispatchTable>,
	addr: SocketAddr,
	peer: SocketAddr,
	datagram: Bytes,
) -> RouteH3 {
	let captured: Arc<FlowGraph> = graph.load_full();
	let kind = captured
		.symbolic()
		.meta
		.listener_kinds
		.get(&addr)
		.copied()
		.unwrap_or(vane_core::ListenerKind::Raw);
	if !matches!(kind, vane_core::ListenerKind::Http) {
		return RouteH3::NotApplicable(datagram);
	}
	let listener_slot = DispatchKey::QuicConnId(quinn_proto::ConnectionId::new(&[]));
	let Some(entry) = dispatch_table.get(&listener_slot) else {
		tracing::trace!(?addr, ?peer, "h3 listener not yet ready; dropping datagram");
		return RouteH3::Routed;
	};
	if let DispatchHandle::Quic(vs) = &**entry {
		vs.enqueue_inbound(peer, datagram);
	}
	RouteH3::Routed
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
///
/// `first_packets` is a length-1 vec on the immediate cold-path; on
/// the pending-peek completion path it carries every buffered
/// Initial datagram in arrival order, which the matched `L4Forward`
/// fetch sends to upstream verbatim before subscribing to the
/// inbound mpsc.
///
/// `sni` is `Some` only on the pending-peek completion path — the
/// pre-extracted SNI is stamped onto `ConnContext.tls.sni` so the
/// matching `tls.sni` predicate evaluates correctly without the
/// listener needing TLS termination.
#[allow(clippy::too_many_arguments)]
async fn handle_cold_path(
	socket: Arc<UdpSocket>,
	peer: SocketAddr,
	first_packets: Vec<Bytes>,
	sni: Option<String>,
	local: SocketAddr,
	entry: NodeId,
	graph: Arc<FlowGraph>,
	dispatch_table: Arc<DispatchTable>,
	verbosity: Arc<VerbosityState>,
	log_sink: Arc<dyn FlowLogSink>,
	force_cancel: CancellationToken,
	_in_flight_guard: InFlightGuard,
) {
	metrics::counter!("vane.requests.total", "listener_addr" => local.to_string()).increment(1);

	let conn_id = crate::listener::next_conn_id();
	let initial_tls = sni.map(|s| TlsInfo { sni: Some(s), ..TlsInfo::default() });
	let conn = Arc::new(ConnContext {
		id: conn_id,
		remote: peer,
		local,
		transport: Transport::Udp,
		entered_at: Instant::now(),
		tls: parking_lot::Mutex::new(initial_tls),
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

	let l4 = L4Conn::Udp(UdpAssoc { socket, peer, first_packets });
	let result = execute(&graph, entry, ExecutorInput::L4(Box::new(l4)), &conn, &mut ctx).await;
	if let Err(e) = result {
		tracing::warn!(error = %e, conn_id = %conn.id, "udp cold path ended with error");
	}
}

fn unix_ms_now() -> u64 {
	std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

async fn bind_udp_with_retry(
	addr: SocketAddr,
	cancel: &CancellationToken,
	max_attempts: u32,
	initial: Duration,
	max: Duration,
) -> Option<UdpSocket> {
	let mut attempt: u32 = 0;
	let mut backoff = initial;
	loop {
		tokio::select! {
			biased;
			() = cancel.cancelled() => return None,
			res = UdpSocket::bind(addr) => match res {
				Ok(s) => return Some(s),
				Err(e) => {
					attempt = attempt.saturating_add(1);
					tracing::warn!(?addr, attempt, error = %e, "udp bind retry");
					if attempt >= max_attempts {
						return None;
					}
					tokio::select! {
						biased;
						() = cancel.cancelled() => return None,
						() = tokio::time::sleep(backoff) => {}
					}
					backoff = (backoff * 2).min(max);
				}
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn advance_pending_peek_drops_on_byte_overflow() {
		// First push exceeds the 16 KiB session cap → Drop without
		// invoking the extractor (the byte budget gate is upstream).
		let state = PendingPeekState::new();
		let oversize = Bytes::from(vec![0u8; PENDING_PEEK_MAX_BYTES + 1]);
		assert!(matches!(advance_pending_peek(&state, &oversize), PendingAdvance::Drop));
	}

	#[test]
	fn advance_pending_peek_drops_on_lifetime_expiry() {
		// `Instant::now() - Duration` panics on underflow; use
		// `checked_sub` to satisfy clippy::unchecked_time_subtraction
		// even though the test is gated to environments where the
		// process has been up long enough for the subtraction to
		// succeed (running an empty `cargo nextest` already exceeds
		// the 2-s offset on every supported platform).
		let aged = Instant::now()
			.checked_sub(PENDING_PEEK_LIFETIME * 2)
			.expect("test instant subtraction within process uptime");
		let state = PendingPeekState { started_at: aged, ..PendingPeekState::new() };
		// Datagram bytes don't matter — the lifetime check runs first.
		let dgram = Bytes::from_static(&[0xc0, 0, 0, 0, 1]);
		assert!(matches!(advance_pending_peek(&state, &dgram), PendingAdvance::Drop));
	}

	#[test]
	fn advance_pending_peek_drops_on_extractor_error() {
		// A short non-QUIC byte string fails `Extractor::push` with
		// `NotInitial` (or `HeaderParse`), which surfaces as Drop.
		let state = PendingPeekState::new();
		let garbage = Bytes::from_static(b"hello");
		assert!(matches!(advance_pending_peek(&state, &garbage), PendingAdvance::Drop));
	}

	#[test]
	fn advance_pending_peek_drops_on_datagram_count_cap() {
		// Pre-fill the buffer to the count cap, then any further push
		// trips the gate before the extractor runs.
		let state = PendingPeekState::new();
		{
			let mut buf = state.datagrams.lock();
			for _ in 0..PENDING_PEEK_MAX_DATAGRAMS {
				buf.push(Bytes::from_static(&[0u8; 16]));
			}
		}
		let dgram = Bytes::from_static(&[0xc0, 0, 0, 0, 1]);
		assert!(matches!(advance_pending_peek(&state, &dgram), PendingAdvance::Drop));
	}

	#[test]
	fn quic_long_header_initial_recognised() {
		// 0xc0 = long-header form, fixed bit set, type=Initial(00).
		assert!(is_quic_long_header_initial(&[0xc0]));
		// Reserved + PN-length bits (still header-protected) must not
		// affect the classification.
		assert!(is_quic_long_header_initial(&[0xc3]));
		assert!(is_quic_long_header_initial(&[0xcf]));
	}

	#[test]
	fn empty_datagram_not_initial() {
		assert!(!is_quic_long_header_initial(&[]));
	}

	#[test]
	fn short_header_not_initial() {
		assert!(!is_quic_long_header_initial(&[0x40]));
	}

	#[test]
	fn long_header_other_packet_types_not_initial() {
		// type=01 (0-RTT)
		assert!(!is_quic_long_header_initial(&[0xd0]));
		// type=10 (Handshake)
		assert!(!is_quic_long_header_initial(&[0xe0]));
		// type=11 (Retry)
		assert!(!is_quic_long_header_initial(&[0xf0]));
	}
}
