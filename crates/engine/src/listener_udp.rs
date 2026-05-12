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

use bytes::Bytes;
use clienthello::{Extractor, PushOutcome};
use dashmap::DashMap;
use parking_lot::Mutex;
use std::sync::Mutex as SyncMutex;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use vane_core::{
	ConnContext, FlowCtx, L4Conn, NodeId, TlsInfo, TrajectoryBuilder, Transport, UdpAssoc,
};

use crate::executor::{ExecutorInput, execute};
use crate::flow_graph::FlowGraph;
use crate::listener_ctx::{AcceptCtx, UdpAcceptCtx};

/// Maximum UDP datagram size. The recv buffer is sized for the IP
/// theoretical max (65535 minus IP+UDP headers, but we round up to
/// 65535 for simplicity — over-sized reads cost ~64 KiB per loop iter
/// which is negligible at expected per-listener traffic).
const MAX_DATAGRAM: usize = 65535;

/// Bounded inbound channel per `L4Forward` session. Full = drop, since
/// UDP is lossy by design and back-pressure to the listener loop would
/// stall every other session sharing the physical socket.
pub const SESSION_INBOUND_CAPACITY: usize = 64;

/// Per `spec/crates/engine.md` § _Multi-packet peek_.
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
/// socket — `spec/crates/engine.md` § _`udp_dispatch`_ locks one `quinn::Endpoint` per `Http` UDP listener, so
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
	Quic(Arc<virtual_socket::VirtualUdpSocket>),
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
	pub in_flight: Arc<SyncMutex<JoinSet<()>>>,
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
pub(crate) async fn run_udp_listener(base: Arc<AcceptCtx>) {
	let bind_policy = tokio_bind_retry::Policy {
		max_attempts: base.bind_cfg.max_bind_attempts,
		initial: base.bind_cfg.bind_backoff_initial,
		max: base.bind_cfg.bind_backoff_max,
		..tokio_bind_retry::Policy::default()
	};
	let Some(socket) = tokio_bind_retry::udp(base.addr, &base.accept_cancel, &bind_policy).await
	else {
		tracing::error!(
			addr = ?base.addr,
			attempts = base.bind_cfg.max_bind_attempts,
			"udp listener bind failed after exhausting retries — giving up on this address",
		);
		return;
	};
	base.bind_ready.store(true, Ordering::Release);
	let socket = Arc::new(socket);
	let ctx = Arc::new(UdpAcceptCtx {
		base,
		socket: Arc::clone(&socket),
		dispatch_table: Arc::new(DispatchTable::new()),
	});

	#[cfg(feature = "h3")]
	setup_h3_endpoint(&ctx);

	// Concurrent pending-peek sessions on this listener. Incremented
	// when a PendingPeek entry is inserted, decremented on removal.
	// Used to enforce the `PENDING_PEEK_MAX_PER_LISTENER` cap without
	// scanning the whole dispatch table per cold-path datagram.
	let pending_count: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(0));

	// Long-lived recv buffer backed by `BytesMut` so each datagram
	// turns into a `Bytes` slice via `split_to(n).freeze()` — no
	// per-datagram `Bytes::copy_from_slice` memcpy. The buffer is
	// re-grown to `MAX_DATAGRAM` after each split so the next
	// `recv_from` sees a full 64 KiB landing zone.
	//
	// `recv_from(&mut [u8])` wants an initialised slice; we drive it
	// through the `read_buf` API instead, which works directly with
	// `BytesMut` and never requires `unsafe` `set_len`.
	let mut buf: bytes::BytesMut = bytes::BytesMut::with_capacity(MAX_DATAGRAM);
	loop {
		tokio::select! {
			biased;
			() = ctx.base.accept_cancel.cancelled() => {
				// Recv loop exits; in-flight cold-path tasks will observe
				// `force_cancel` via their FlowCtx (drive_byte_tunnel arm
				// propagates into the forwarder's cancel token).
				return;
			}
			recv = recv_one_datagram(&socket, &mut buf) => {
				let (datagram, peer) = match recv {
					Ok(v) => v,
					Err(e) => {
						tracing::debug!(addr = ?ctx.base.addr, ?e, "udp recv_from error; continuing");
						continue;
					}
				};

				if let Some(entry) = ctx.dispatch_table.get(&DispatchKey::Peer(peer)) {
					dispatch_existing_entry(&entry, peer, datagram);
					continue;
				}

				let datagram = match advance_existing_pending_peek(&ctx, peer, datagram, &pending_count) {
					PendingPeekDispatch::Handled => continue,
					PendingPeekDispatch::FellThrough(d) => d,
				};

				#[cfg(feature = "h3")]
				let datagram = match try_route_to_h3(&ctx, peer, datagram) {
					RouteH3::Routed => continue,
					RouteH3::NotApplicable(d) => d,
				};

				dispatch_cold_datagram(&ctx, peer, datagram, &pending_count);
			}
		}
	}
}

/// Bring up the per-listener H3 stack on UDP+Http listeners: a
/// `quinn::Endpoint` wrapping a `VirtualUdpSocket`, registered in the
/// dispatch table under the well-known `QuicConnId(empty)` slot — see
/// `spec/crates/engine.md` § _`udp_dispatch`_. No-op when the listener
/// is non-Http or has no TLS config.
#[cfg(feature = "h3")]
/// Receive one datagram into the long-lived `BytesMut`, return the
/// freshly-split `Bytes` view + peer address. The buffer is grown back
/// to `MAX_DATAGRAM` capacity after the split so the next call sees a
/// full landing zone without re-allocating.
///
/// This avoids the `Bytes::copy_from_slice(&buf[..n])` memcpy on the
/// prior `Vec<u8>` path; `BytesMut::split_to(n).freeze()` is a
/// refcount transfer.
async fn recv_one_datagram(
	socket: &tokio::net::UdpSocket,
	buf: &mut bytes::BytesMut,
) -> std::io::Result<(Bytes, SocketAddr)> {
	// Reserve full landing-zone capacity (no-op when the buffer is
	// already at MAX_DATAGRAM after the previous freeze).
	if buf.capacity() < MAX_DATAGRAM {
		buf.reserve(MAX_DATAGRAM - buf.len());
	}
	// `recv_buf_from` writes directly into the spare capacity and
	// advances the cursor to `len + n` — no manual `set_len` and no
	// `unsafe`. Returns `(n, peer)`; we then split off the bytes we
	// just received and freeze them into a refcounted `Bytes`.
	let (n, peer) = socket.recv_buf_from(buf).await?;
	let datagram = buf.split_to(n).freeze();
	Ok((datagram, peer))
}

fn setup_h3_endpoint(ctx: &Arc<UdpAcceptCtx>) {
	let captured = ctx.base.graph.load_full();
	let kind = captured
		.symbolic()
		.meta
		.listener_kinds
		.get(&ctx.base.addr)
		.copied()
		.unwrap_or(vane_core::ListenerKind::Raw);
	if !matches!(kind, vane_core::ListenerKind::Http) {
		return;
	}
	let Some(tls_cfg) = captured.listener_tls(&ctx.base.addr).cloned() else {
		tracing::warn!(
			addr = ?ctx.base.addr,
			"udp Http listener has no listener_tls config; H3 requires TLS — skipping H3 setup",
		);
		return;
	};
	match crate::h3::listener::spawn_h3_endpoint(ctx, &tls_cfg) {
		Ok(()) => tracing::info!(addr = ?ctx.base.addr, "h3 listener up"),
		Err(e) => tracing::error!(addr = ?ctx.base.addr, error = %e, "h3 endpoint setup failed"),
	}
}

/// Route an inbound datagram that matched an existing dispatch-table
/// entry. The handle variant decides the destination: live `L4Forward`
/// session inbound channel (bounded; drop on full per spec since UDP
/// is lossy and back-pressure to the listener loop would stall every
/// peer sharing the socket), the listener-level QUIC virtual socket,
/// or the unreachable Peer(_) → PendingPeek invariant violation.
fn dispatch_existing_entry(handle: &DispatchHandle, peer: SocketAddr, datagram: Bytes) {
	match handle {
		DispatchHandle::L4Forward(session) => {
			if session.inbound_tx.try_send(datagram).is_err() {
				tracing::warn!(
					target: "udp_forward",
					?peer,
					"udp session inbound channel full or closed; dropping datagram",
				);
			}
		}
		DispatchHandle::PendingPeek(_) => {
			// `Peer(peer)` cannot legally point at a PendingPeek handle
			// — pending-peek lives under the `PendingPeek(peer)` key.
			// This branch is unreachable in correct code; trace + drop.
			tracing::warn!(?peer, "dispatch table internal invariant: Peer(_) → PendingPeek; dropping",);
		}
		#[cfg(feature = "h3")]
		DispatchHandle::Quic(virtual_socket) => {
			virtual_socket.enqueue_inbound(peer, datagram);
		}
	}
}

/// Outcome of [`advance_existing_pending_peek`]: either the datagram
/// was consumed by an in-progress pending-peek session (caller
/// `continue`s), or no session existed and the original datagram is
/// returned for the cold-path / H3 fan-in branches downstream.
enum PendingPeekDispatch {
	Handled,
	FellThrough(Bytes),
}

/// Drive the existing pending-peek session for `peer` (per
/// `spec/crates/engine.md` § _Multi-packet peek_). When the session
/// completes (`Sni`), the buffered datagrams are replayed via
/// `spawn_cold_path`. `NeedMore` keeps the session live; `Drop` evicts
/// the entry and decrements the counter. Returns whether the caller
/// should `continue` past the rest of the recv-loop dispatch chain.
fn advance_existing_pending_peek(
	ctx: &Arc<UdpAcceptCtx>,
	peer: SocketAddr,
	datagram: Bytes,
	pending_count: &Arc<AtomicUsize>,
) -> PendingPeekDispatch {
	let pending_key = DispatchKey::PendingPeek(peer);
	let Some(entry) = ctx.dispatch_table.get(&pending_key) else {
		return PendingPeekDispatch::FellThrough(datagram);
	};
	let DispatchHandle::PendingPeek(state) = &**entry else {
		return PendingPeekDispatch::FellThrough(datagram);
	};
	let state = Arc::clone(state);
	drop(entry);
	match advance_pending_peek(&state, &datagram) {
		PendingAdvance::Sni(sni) => {
			let datagrams = drain_pending(&state, datagram);
			if ctx.dispatch_table.remove(&pending_key).is_some() {
				pending_count.fetch_sub(1, Ordering::Relaxed);
			}
			spawn_cold_path(ctx, peer, datagrams, Some(sni));
		}
		PendingAdvance::NeedMore => {
			// Buffered for later datagram; no spawn.
		}
		PendingAdvance::Drop(reason) => {
			record_peek_dropped(ctx.base.addr, reason);
			if ctx.dispatch_table.remove(&pending_key).is_some() {
				pending_count.fetch_sub(1, Ordering::Relaxed);
			}
		}
	}
	PendingPeekDispatch::Handled
}

/// Cold-path entry for a datagram with no existing dispatch state.
/// Capture a graph snapshot per-datagram so reload cannot pull the rug,
/// then decide between starting a pending-peek session (multi-packet
/// QUIC SNI extraction) and immediate cold-path spawn.
fn dispatch_cold_datagram(
	ctx: &Arc<UdpAcceptCtx>,
	peer: SocketAddr,
	datagram: Bytes,
	pending_count: &Arc<AtomicUsize>,
) {
	let captured: Arc<FlowGraph> = ctx.base.graph.load_full();
	let Some(entry) = captured.symbolic().entries.get(&ctx.base.addr).copied() else {
		tracing::debug!(
			addr = ?ctx.base.addr,
			?peer,
			"udp cold path: no entry in active graph; dropping datagram",
		);
		return;
	};
	if !(captured.needs_pending_peek(ctx.base.addr, entry) && is_quic_long_header_initial(&datagram))
	{
		spawn_cold_path(ctx, peer, vec![datagram], None);
		return;
	}
	if pending_count.load(Ordering::Relaxed) >= PENDING_PEEK_MAX_PER_LISTENER {
		// `spec/crates/engine.md` § _Multi-packet peek_: silent drop past
		// the per-listener cap. Operators see the drop only via the
		// `vane.listener.peek.dropped` counter; no per-drop log to
		// avoid amplifying a flood.
		record_peek_dropped(ctx.base.addr, PeekDropReason::PerListenerCap);
		return;
	}
	let state = Arc::new(PendingPeekState::new());
	match advance_pending_peek(&state, &datagram) {
		PendingAdvance::Sni(sni) => {
			let datagrams = drain_pending(&state, datagram);
			spawn_cold_path(ctx, peer, datagrams, Some(sni));
		}
		PendingAdvance::NeedMore => {
			let pending_key = DispatchKey::PendingPeek(peer);
			let handle = Arc::new(DispatchHandle::PendingPeek(Arc::clone(&state)));
			if ctx.dispatch_table.insert(pending_key, handle).is_none() {
				pending_count.fetch_add(1, Ordering::Relaxed);
			}
		}
		PendingAdvance::Drop(reason) => {
			// First datagram failed parsing — record the bound that
			// tripped, then fall back to immediate cold-path entry
			// with the raw datagram, mirroring spec behaviour of
			// "not Initial → fall through".
			record_peek_dropped(ctx.base.addr, reason);
			spawn_cold_path(ctx, peer, vec![datagram], None);
		}
	}
}

/// Outcome of a single [`advance_pending_peek`] step.
enum PendingAdvance {
	Sni(String),
	NeedMore,
	Drop(PeekDropReason),
}

/// Why a pending-peek session was rejected. Each variant maps to a
/// distinct `reason` label on the `vane.listener.peek.dropped` counter
/// so operators can localise which bound trips most often in
/// production. Keep the label set bounded — each new variant must
/// land in the prom-cardinality-cap admit table.
#[derive(Copy, Clone, Debug)]
enum PeekDropReason {
	LifetimeExpired,
	MaxBytes,
	MaxDatagrams,
	ExtractError,
	PerListenerCap,
}

impl PeekDropReason {
	const fn label(self) -> &'static str {
		match self {
			Self::LifetimeExpired => "lifetime_expired",
			Self::MaxBytes => "max_bytes",
			Self::MaxDatagrams => "max_datagrams",
			Self::ExtractError => "extract_error",
			Self::PerListenerCap => "per_listener_cap",
		}
	}
}

fn record_peek_dropped(listener_addr: SocketAddr, reason: PeekDropReason) {
	// Label cardinality stays bounded: `listener_port` is a u16 (one
	// label value per bound port) and `reason` is the closed enum
	// above. See `crates/lib/prom-cardinality-cap`.
	metrics::counter!(
		"vane.listener.peek.dropped",
		"listener_port" => listener_addr.port().to_string(),
		"reason" => reason.label(),
	)
	.increment(1);
}

/// Push one datagram into the `PendingPeekState` and apply the spec's
/// bound checks. Bounds: bytes ≤ 16 KiB, datagram count ≤ 8, lifetime
/// ≤ 1 s. Exceeding any bound, or an extraction error, returns
/// [`PendingAdvance::Drop`] so the caller removes the entry.
fn advance_pending_peek(state: &PendingPeekState, datagram: &Bytes) -> PendingAdvance {
	if state.started_at.elapsed() > PENDING_PEEK_LIFETIME {
		return PendingAdvance::Drop(PeekDropReason::LifetimeExpired);
	}
	let new_bytes = state.bytes.load(Ordering::Relaxed).saturating_add(datagram.len());
	if new_bytes > PENDING_PEEK_MAX_BYTES {
		return PendingAdvance::Drop(PeekDropReason::MaxBytes);
	}
	{
		let mut buf = state.datagrams.lock();
		if buf.len() >= PENDING_PEEK_MAX_DATAGRAMS {
			return PendingAdvance::Drop(PeekDropReason::MaxDatagrams);
		}
		buf.push(datagram.clone());
	}
	state.bytes.store(new_bytes, Ordering::Relaxed);

	match state.extractor.lock().push(datagram) {
		Ok(PushOutcome::Sni(s)) => PendingAdvance::Sni(s),
		Ok(PushOutcome::NeedMore) => PendingAdvance::NeedMore,
		Err(clienthello::Error::UnsupportedVersion(version)) => {
			// QUIC v2 (0x6b33_43cf) and any other version probe lands
			// here. Surface a dedicated counter so operators can watch
			// the deployed client mix in their dashboards; the version
			// nibble is a small bounded set (v1 doesn't reach this
			// branch, v2 is the realistic case, anything else is an
			// experiment or attacker) so the label cardinality stays
			// safe. Remove once clienthello learns to extract from v2.
			metrics::counter!(
				"vane.peek.quic.unsupported_version",
				"version" => format!("{version:#010x}"),
			)
			.increment(1);
			PendingAdvance::Drop(PeekDropReason::ExtractError)
		}
		Err(_) => PendingAdvance::Drop(PeekDropReason::ExtractError),
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

fn spawn_cold_path(
	ctx: &Arc<UdpAcceptCtx>,
	peer: SocketAddr,
	first_packets: Vec<Bytes>,
	sni: Option<String>,
) {
	let captured: Arc<FlowGraph> = ctx.base.graph.load_full();
	let Some(entry) = captured.symbolic().entries.get(&ctx.base.addr).copied() else {
		tracing::debug!(
			addr = ?ctx.base.addr,
			?peer,
			"udp cold path: no entry in active graph at spawn time; dropping datagram",
		);
		return;
	};
	ctx.base.in_flight_count.fetch_add(1, Ordering::Relaxed);
	let in_flight_guard = InFlightGuard(Arc::clone(&ctx.base.in_flight_count));
	ctx.base.in_flight.lock().expect("in_flight mutex poisoned").spawn(handle_cold_path(
		Arc::clone(ctx),
		peer,
		first_packets,
		sni,
		entry,
		captured,
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
/// `QuicConnId(empty)` slot — `spec/crates/engine.md` § _`udp_dispatch`_ holds one virtual socket per listener, so
/// the empty-CID slot is the listener's single QUIC fan-in entry
/// rather than a per-connection key.
#[cfg(feature = "h3")]
fn try_route_to_h3(ctx: &Arc<UdpAcceptCtx>, peer: SocketAddr, datagram: Bytes) -> RouteH3 {
	let captured: Arc<FlowGraph> = ctx.base.graph.load_full();
	let kind = captured
		.symbolic()
		.meta
		.listener_kinds
		.get(&ctx.base.addr)
		.copied()
		.unwrap_or(vane_core::ListenerKind::Raw);
	if !matches!(kind, vane_core::ListenerKind::Http) {
		return RouteH3::NotApplicable(datagram);
	}
	let listener_slot = DispatchKey::QuicConnId(quinn_proto::ConnectionId::new(&[]));
	let Some(entry) = ctx.dispatch_table.get(&listener_slot) else {
		tracing::trace!(addr = ?ctx.base.addr, ?peer, "h3 listener not yet ready; dropping datagram");
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
async fn handle_cold_path(
	ctx: Arc<UdpAcceptCtx>,
	peer: SocketAddr,
	first_packets: Vec<Bytes>,
	sni: Option<String>,
	entry: NodeId,
	graph: Arc<FlowGraph>,
	_in_flight_guard: InFlightGuard,
) {
	let local = ctx.base.addr;
	// Same cardinality discipline as the TCP path (see
	// `crates/engine/src/listener.rs`): port-only label keeps the
	// admit-table footprint bounded.
	metrics::counter!("vane.requests.total", "listener_port" => local.port().to_string())
		.increment(1);

	let conn_id = crate::listener::next_conn_id();
	let initial_tls = sni.map(|s| TlsInfo { sni: Some(Arc::from(s)), ..TlsInfo::default() });
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
	conn.user.lock().insert(Arc::clone(&ctx.dispatch_table));

	let span = tracing::info_span!("udp_conn", id = %conn.id);
	let mut flow_ctx = FlowCtx {
		span,
		log: Arc::clone(&ctx.base.log_sink),
		cancel: ctx.base.force_cancel.clone(),
		accept_cancel: ctx.base.accept_cancel.clone(),
		verbosity: ctx.base.verbosity.current(),
		trajectory: TrajectoryBuilder::new(conn.id, entry, unix_ms_now()),
	};

	let l4 = L4Conn::Udp(UdpAssoc { socket: Arc::clone(&ctx.socket), peer, first_packets });
	let result = execute(&graph, entry, ExecutorInput::L4(Box::new(l4)), &conn, &mut flow_ctx).await;
	if let Err(e) = result {
		tracing::warn!(error = %e, conn_id = %conn.id, "udp cold path ended with error");
	}
}

fn unix_ms_now() -> u64 {
	std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
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
		assert!(matches!(advance_pending_peek(&state, &oversize), PendingAdvance::Drop(_)));
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
		assert!(matches!(advance_pending_peek(&state, &dgram), PendingAdvance::Drop(_)));
	}

	#[test]
	fn advance_pending_peek_drops_on_extractor_error() {
		// A short non-QUIC byte string fails `Extractor::push` with
		// `NotInitial` (or `HeaderParse`), which surfaces as Drop.
		let state = PendingPeekState::new();
		let garbage = Bytes::from_static(b"hello");
		assert!(matches!(advance_pending_peek(&state, &garbage), PendingAdvance::Drop(_)));
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
		assert!(matches!(advance_pending_peek(&state, &dgram), PendingAdvance::Drop(_)));
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
