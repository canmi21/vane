//! TCP accept loop + bind-retry + cancellation tier + soft drain.
//!
//! See `spec/architecture/01-topology.md` § _Listener lifecycle_ /
//! _Bind_ / _Accept loop_ / _Shutdown_, and `spec/architecture/06-l4.md`.
//! Features: S1-13, S1-14.
//!
//! Shape of the cancellation tier (01-topology.md § _Listener lifecycle_
//! step 3 — listeners removed):
//!
//! 1. `accept_cancel` fires → accept loop stops binding new connections.
//! 2. The shutdown driver waits up to `drain_timeout` for in-flight
//!    connections (held in a `JoinSet`) to complete naturally.
//! 3. On timeout the driver fires `force_cancel`, which is the
//!    `CancellationToken` every per-connection `FlowCtx` was built with.
//!    `ctx.cancel.cancelled()` propagates into long-lived terminators —
//!    notably `Terminator::ByteTunnel` (executor's `tokio::select!` on the
//!    cancel token surfaces `CloseReason::Cancelled`).
//! 4. After a short secondary grace window any still-alive task is aborted.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use arc_swap::ArcSwap;
use parking_lot::Mutex;
use tokio::net::{TcpListener, TcpSocket, TcpStream};
use tokio::sync::Mutex as AsyncMutex;
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;
use vane_core::{
	ConnContext, ConnId, FlowCtx, FlowLogSink, L4Conn, NodeId, TrajectoryBuilder, Transport,
};

use crate::executor::{ExecutorInput, execute};
use crate::flow_graph::FlowGraph;
use crate::verbosity::VerbosityState;

// 01-topology.md § _Bind_ / _Daemon lifecycle_:
//   max_bind_attempts default 10, exponential backoff 100ms → 5s cap.
//   drain_timeout default 30s (pass into `shutdown`).
// TODO(s1-config): wire from config when 09-config.md lands; for now these
// are MVP-hardcoded constants exposed only for tests via the shadowing
// `_for_test` helpers below.
const MAX_BIND_ATTEMPTS: u32 = 10;
const BIND_BACKOFF_INITIAL: Duration = Duration::from_millis(100);
const BIND_BACKOFF_MAX: Duration = Duration::from_secs(5);
const FORCE_CANCEL_GRACE: Duration = Duration::from_secs(5);
/// Per-listener drain budget when an address is removed across a hot
/// reload. Mirrors the SIGTERM drain default — operators can rely on
/// the same time bound for both shutdown and removed-listener drain.
const RECONCILE_DRAIN_TIMEOUT: Duration = Duration::from_secs(30);
const TCP_LISTEN_BACKLOG: u32 = 1024;

static NEXT_CONN_ID: AtomicU64 = AtomicU64::new(1);

fn next_conn_id() -> ConnId {
	ConnId(NEXT_CONN_ID.fetch_add(1, Ordering::Relaxed))
}

fn unix_ms_now() -> u64 {
	SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
		.unwrap_or_default()
}

/// Per-(transport, address) listener registry. Today: TCP only.
///
/// Listener configuration changes only occur at boot and reload
/// (01-topology.md § _Listener lifecycle_). `start` is idempotent on
/// duplicate addresses (already-running keys are skipped with a warn —
/// reload's reconcile pass lands in S1-28 and replaces this).
pub struct ListenerSet {
	running: Mutex<HashMap<SocketAddr, ListenerHandle>>,
}

struct ListenerHandle {
	accept_cancel: CancellationToken,
	force_cancel: CancellationToken,
	in_flight: Arc<AsyncMutex<JoinSet<()>>>,
	join: JoinHandle<()>,
}

impl ListenerSet {
	#[must_use]
	pub fn new() -> Self {
		Self { running: Mutex::new(HashMap::new()) }
	}

	/// Spawn one TCP accept task per `SocketAddr` in the **initial
	/// snapshot** of `graph`. Each accept loop captures the
	/// `Arc<ArcSwap<FlowGraph>>` and resolves the entry `NodeId` per
	/// accepted connection by looking the listener's local
	/// `SocketAddr` up in the active graph's `entries` map.
	///
	/// Per-accept lookup (rather than a baked-in `NodeId` from boot) is
	/// required because `NodeId` is a slab index that
	/// `compile/lower.rs::lower_port` reassigns from scratch on every
	/// recompile — the index in the post-reload graph need not name the
	/// same logical node as the pre-reload graph (09-config.md
	/// § _`NodeId` stability across reloads_). `SocketAddr` is the
	/// stable identifier; the lookup costs an `entries.get(&addr)` per
	/// connection.
	///
	/// If the active graph no longer has an entry for `addr` (operator
	/// removed the rule that owned the listener), the stream is dropped
	/// immediately and the client sees TCP RST. The accept socket itself
	/// stays bound for now — listener-set diffing on reload is a separate
	/// future change; today, introducing a new `listen` port still
	/// requires a daemon restart.
	///
	/// Spawning is fire-and-forget; bind failures and accept-loop errors
	/// surface only via `tracing` events. The handle stored in `running`
	/// drives the shutdown protocol.
	#[allow(clippy::needless_pass_by_value)]
	pub fn start(
		&self,
		graph: Arc<ArcSwap<FlowGraph>>,
		verbosity: Arc<VerbosityState>,
		log_sink: Arc<dyn FlowLogSink>,
	) {
		let addrs: Vec<SocketAddr> = {
			let initial = graph.load_full();
			initial.symbolic().entries.keys().copied().collect()
		};

		// Every entry binds. The lower pass guarantees entry nodes start in
		// phase L4Raw (02-flow.md § _Phase state machine_), so the executor
		// always sees the L4 input shape it expects. L4 → L7 transitions
		// happen inside the executor at `Node::Upgrade`, which now hands
		// the stream to `drive_h1_server` for hyper to decode.
		for addr in addrs {
			let mut running = self.running.lock();
			if running.contains_key(&addr) {
				tracing::warn!(?addr, "listener already running for this address; skipping");
				continue;
			}

			let accept_cancel = CancellationToken::new();
			let force_cancel = CancellationToken::new();
			let in_flight = Arc::new(AsyncMutex::new(JoinSet::new()));

			let join = tokio::spawn(run_accept_loop(
				addr,
				Arc::clone(&graph),
				Arc::clone(&verbosity),
				Arc::clone(&log_sink),
				accept_cancel.clone(),
				force_cancel.clone(),
				Arc::clone(&in_flight),
			));

			running.insert(addr, ListenerHandle { accept_cancel, force_cancel, in_flight, join });
		}
	}

	/// Whether a listener is currently running for `addr`. Useful for tests
	/// that observe lifecycle transitions.
	#[must_use]
	pub fn is_running(&self, addr: &SocketAddr) -> bool {
		self.running.lock().contains_key(addr)
	}

	/// Number of running listeners.
	#[must_use]
	pub fn len(&self) -> usize {
		self.running.lock().len()
	}

	/// Whether no listeners are currently running.
	#[must_use]
	pub fn is_empty(&self) -> bool {
		self.running.lock().is_empty()
	}

	/// Soft-drain shutdown per 01-topology.md § _Listener lifecycle_ step 3.
	///
	/// Stages:
	/// 1. Fire every accept-loop cancel → accept loops drop their listening
	///    socket and exit. No new connections enter the in-flight set.
	/// 2. Per listener: wait `drain_timeout` for the in-flight `JoinSet` to
	///    drain. In-flight tasks see no force-cancel signal yet — they get
	///    to finish naturally.
	/// 3. On per-listener timeout: fire `force_cancel`. Per-connection
	///    `FlowCtx::cancel` is a clone of this token; long-lived terminators
	///    (notably `Terminator::ByteTunnel`'s `tokio::select!`) observe it
	///    and unwind, sending `CloseReason::Cancelled` through the tunnel's
	///    `close_reason_tx`.
	/// 4. After a short secondary grace window, abort anything still alive
	///    so the call returns within `drain_timeout + FORCE_CANCEL_GRACE`.
	///
	/// `&self` so the registry can be reached through `Arc<ListenerSet>`
	/// without consuming the wrapper — the watcher's reload pipeline
	/// holds one such `Arc` for `reconcile()` and the daemon main holds
	/// another for shutdown. Internally `running.lock().drain()` empties
	/// the registry as a side effect, so a second `shutdown` call is a
	/// cheap no-op.
	pub async fn shutdown(&self, drain_timeout: Duration) {
		let handles: Vec<(SocketAddr, ListenerHandle)> = {
			let mut running = self.running.lock();
			running.drain().collect()
		};

		// Stage 1: fire all accept_cancels at once so accept loops stop
		// admitting new work in parallel, not one-by-one.
		for (_, h) in &handles {
			h.accept_cancel.cancel();
		}

		for (addr, handle) in handles {
			let ListenerHandle { accept_cancel: _, force_cancel, in_flight, join } = handle;

			// Wait for the accept loop to wind down. It only needs to
			// complete one select! cycle to observe accept_cancel; if the
			// task panicked, JoinHandle returns Err and we proceed anyway.
			let _ = join.await;

			// Stage 2: wait drain_timeout for in-flight to clear naturally.
			if tokio::time::timeout(drain_timeout, drain_in_flight(&in_flight)).await.is_ok() {
				tracing::debug!(?addr, "in-flight drain completed within timeout");
			} else {
				tracing::warn!(
					?addr,
					?drain_timeout,
					"drain timed out — firing force_cancel for in-flight",
				);
				// Stage 3: signal in-flight executors to unwind.
				force_cancel.cancel();
				// Secondary grace window for cooperative shutdown.
				let _ = tokio::time::timeout(FORCE_CANCEL_GRACE, drain_in_flight(&in_flight)).await;
				// Stage 4: anything still alive gets the abort hammer.
				let mut g = in_flight.lock().await;
				g.abort_all();
				while g.join_next().await.is_some() {}
			}
		}
	}

	/// Diff the active graph's `entries` keys against currently bound
	/// listeners and bring the registry up to date with the post-reload
	/// snapshot.
	///
	/// - **Added** addresses (in new graph, not currently bound): spawn
	///   a fresh `run_accept_loop` task with the same wiring as
	///   [`Self::start`], including bind-retry.
	/// - **Removed** addresses (currently bound, not in new graph): pop
	///   the handle and `tokio::spawn` a background drain that fires
	///   `accept_cancel`, waits up to [`RECONCILE_DRAIN_TIMEOUT`] for
	///   in-flight connections to finish, then escalates to
	///   `force_cancel` and abort if needed.
	/// - **Unchanged** addresses: untouched. The accept loop's per-accept
	///   `entries.get(&addr)` lookup picks up the new graph's `NodeId`
	///   on the next accepted connection (09-config.md § _`NodeId`
	///   stability across reloads_).
	///
	/// Returns immediately — the per-listener drain runs in the
	/// background so file-watcher reloads never stall on long-lived
	/// `ByteTunnel` connections. Caller invokes this after a successful
	/// `ArcSwap::store` of a reload's new graph; in-flight connections
	/// accepted before this call retain their captured
	/// `Arc<FlowGraph>` and run to completion regardless of the diff.
	#[allow(clippy::needless_pass_by_value)]
	pub fn reconcile(
		&self,
		graph: Arc<ArcSwap<FlowGraph>>,
		verbosity: Arc<VerbosityState>,
		log_sink: Arc<dyn FlowLogSink>,
	) {
		let target: std::collections::HashSet<SocketAddr> = {
			let g = graph.load_full();
			g.symbolic().entries.keys().copied().collect()
		};

		let mut running = self.running.lock();
		let current: std::collections::HashSet<SocketAddr> = running.keys().copied().collect();

		// Removed: collect addresses up front so we can `remove()` from
		// `running` inside the loop without aliasing.
		let removed: Vec<SocketAddr> = current.difference(&target).copied().collect();
		for addr in removed {
			if let Some(handle) = running.remove(&addr) {
				tracing::info!(?addr, "reconcile: removing listener");
				tokio::spawn(drain_handle_async(addr, handle));
			}
		}

		// Added: same wiring as `start()`. `run_accept_loop` does its
		// own bind-with-retry; failure surfaces via `tracing::error!`
		// without poisoning the rest of the registry.
		let added: Vec<SocketAddr> = target.difference(&current).copied().collect();
		for addr in added {
			tracing::info!(?addr, "reconcile: adding listener");
			let accept_cancel = CancellationToken::new();
			let force_cancel = CancellationToken::new();
			let in_flight = Arc::new(AsyncMutex::new(JoinSet::new()));
			let join = tokio::spawn(run_accept_loop(
				addr,
				Arc::clone(&graph),
				Arc::clone(&verbosity),
				Arc::clone(&log_sink),
				accept_cancel.clone(),
				force_cancel.clone(),
				Arc::clone(&in_flight),
			));
			running.insert(addr, ListenerHandle { accept_cancel, force_cancel, in_flight, join });
		}
		// Unchanged addresses: the per-accept `entries.get(&addr)` in
		// `run_accept_loop` already picks up the post-swap NodeId on
		// the next connection — nothing to do here.
	}
}

/// Background drain of a removed listener's handle. Mirrors the
/// per-listener stages of [`ListenerSet::shutdown`] but runs as a
/// `tokio::spawn`'d task so [`ListenerSet::reconcile`] returns
/// immediately.
async fn drain_handle_async(addr: SocketAddr, handle: ListenerHandle) {
	let ListenerHandle { accept_cancel, force_cancel, in_flight, join } = handle;

	// Stop accepting new connections.
	accept_cancel.cancel();
	let _ = join.await;

	// Soft drain.
	if tokio::time::timeout(RECONCILE_DRAIN_TIMEOUT, drain_in_flight(&in_flight)).await.is_ok() {
		tracing::info!(?addr, "reconcile drain complete");
		return;
	}

	tracing::warn!(?addr, "reconcile drain timed out; firing force_cancel for in-flight");
	force_cancel.cancel();
	let _ = tokio::time::timeout(FORCE_CANCEL_GRACE, drain_in_flight(&in_flight)).await;
	let mut g = in_flight.lock().await;
	g.abort_all();
	while g.join_next().await.is_some() {}
	tracing::info!(?addr, "reconcile drain complete (forced)");
}

impl Default for ListenerSet {
	fn default() -> Self {
		Self::new()
	}
}

async fn drain_in_flight(set: &AsyncMutex<JoinSet<()>>) {
	// The accept loop has exited before this is called (`join.await`
	// completed in `shutdown`), so no new tasks enter the set. Holding the
	// lock across `join_next` is safe — there are no contending spawners.
	let mut g = set.lock().await;
	while g.join_next().await.is_some() {}
}

async fn run_accept_loop(
	addr: SocketAddr,
	graph: Arc<ArcSwap<FlowGraph>>,
	verbosity: Arc<VerbosityState>,
	log_sink: Arc<dyn FlowLogSink>,
	accept_cancel: CancellationToken,
	force_cancel: CancellationToken,
	in_flight: Arc<AsyncMutex<JoinSet<()>>>,
) {
	let Some(listener) = bind_with_retry(addr, &accept_cancel, MAX_BIND_ATTEMPTS).await else {
		tracing::error!(
			?addr,
			attempts = MAX_BIND_ATTEMPTS,
			"listener bind failed after exhausting retries — giving up on this address",
		);
		return;
	};

	loop {
		tokio::select! {
			biased;
			() = accept_cancel.cancelled() => return,
			accepted = listener.accept() => {
				let (stream, remote) = match accepted {
					Ok(s) => s,
					Err(e) => {
						// EMFILE / ENFILE / etc. — back off and resume.
						tracing::warn!(?addr, ?e, "accept failed; backing off");
						let cancelled = backoff_sleep(BIND_BACKOFF_INITIAL, &accept_cancel).await;
						if cancelled {
							return;
						}
						continue;
					}
				};

				// Per-accept snapshot of the active graph + per-accept
				// entry lookup by `addr`. `NodeId` is a slab index that
				// `lower_port` reassigns on every recompile, so a baked-in
				// boot-time `NodeId` would route post-reload connections to
				// the wrong logical entry (09-config.md § _NodeId stability
				// across reloads_). The captured `Arc<FlowGraph>` then
				// travels with this connection to natural completion;
				// `ArcSwap::store` from the reload pipeline never disturbs
				// in-flight work.
				let captured: Arc<FlowGraph> = graph.load_full();
				let Some(entry) = captured.symbolic().entries.get(&addr).copied() else {
					// Active graph has no entry for this listener: a reload
					// removed the rule that owned the address. Drop the
					// stream so the client sees TCP RST. The accept socket
					// itself stays bound until the daemon restarts (no
					// listener-set diff yet).
					tracing::debug!(?addr, "no entry in active graph; dropping connection");
					drop(stream);
					continue;
				};

				let verbosity = Arc::clone(&verbosity);
				let log_sink = Arc::clone(&log_sink);
				let force = force_cancel.clone();
				in_flight.lock().await.spawn(handle_connection(
					stream, remote, addr, entry, captured, verbosity, log_sink, force,
				));
			}
		}
	}
}

/// Sleep for `delay`, returning `true` if the sleep was cut short by
/// `cancel.cancelled()`.
async fn backoff_sleep(delay: Duration, cancel: &CancellationToken) -> bool {
	tokio::select! {
		biased;
		() = cancel.cancelled() => true,
		() = tokio::time::sleep(delay) => false,
	}
}

#[allow(clippy::too_many_arguments)]
async fn handle_connection(
	stream: TcpStream,
	remote: SocketAddr,
	local: SocketAddr,
	entry: NodeId,
	graph: Arc<FlowGraph>,
	verbosity: Arc<VerbosityState>,
	log_sink: Arc<dyn FlowLogSink>,
	force_cancel: CancellationToken,
) {
	let conn = Arc::new(ConnContext {
		id: next_conn_id(),
		remote,
		local,
		transport: Transport::Tcp,
		entered_at: Instant::now(),
		tls: parking_lot::Mutex::new(None),
		http_version: std::sync::OnceLock::new(),
		user: parking_lot::Mutex::new(http::Extensions::new()),
	});

	let span = tracing::info_span!("conn", id = %conn.id);
	let mut ctx = FlowCtx {
		span,
		log: log_sink,
		cancel: force_cancel,
		verbosity: verbosity.current(),
		trajectory: TrajectoryBuilder::new(conn.id, entry, unix_ms_now()),
	};

	let result =
		execute(&graph, entry, ExecutorInput::L4(Box::new(L4Conn::Tcp(stream))), &conn, &mut ctx).await;

	if let Err(e) = result {
		tracing::warn!(error = %e, conn_id = %conn.id, "connection ended with error");
	}
}

/// Bind-with-retry per 01-topology.md § _Bind_:
/// - `SO_REUSEADDR` on (best-effort).
/// - Exponential backoff `100ms → 5s cap`.
/// - Up to `max_attempts` tries.
/// - Honors `cancel`: if cancellation fires during a backoff window the
///   function aborts and returns `None`.
///
/// `max_attempts` is parametric so tests can drive the give-up branch
/// without burning real backoff time. Production calls use
/// `MAX_BIND_ATTEMPTS`.
async fn bind_with_retry(
	addr: SocketAddr,
	cancel: &CancellationToken,
	max_attempts: u32,
) -> Option<TcpListener> {
	let mut delay = BIND_BACKOFF_INITIAL;
	for attempt in 0..max_attempts {
		if cancel.is_cancelled() {
			return None;
		}
		let socket_res = match addr {
			SocketAddr::V4(_) => TcpSocket::new_v4(),
			SocketAddr::V6(_) => TcpSocket::new_v6(),
		};
		let socket = match socket_res {
			Ok(s) => s,
			Err(e) => {
				tracing::warn!(?addr, attempt, ?e, "tcp socket creation failed");
				if backoff_sleep(delay, cancel).await {
					return None;
				}
				delay = (delay * 2).min(BIND_BACKOFF_MAX);
				continue;
			}
		};
		// Best-effort REUSEADDR; ignore failure (some platforms require root).
		let _ = socket.set_reuseaddr(true);
		match socket.bind(addr) {
			Ok(()) => match socket.listen(TCP_LISTEN_BACKLOG) {
				Ok(l) => return Some(l),
				Err(e) => {
					tracing::warn!(?addr, attempt, ?e, "tcp listen failed");
				}
			},
			Err(e) => {
				tracing::warn!(?addr, attempt, ?e, "tcp bind failed");
			}
		}
		if backoff_sleep(delay, cancel).await {
			return None;
		}
		delay = (delay * 2).min(BIND_BACKOFF_MAX);
	}
	None
}

/// Test-only entry point: drive `bind_with_retry` with a custom attempt
/// cap. Tests use this to exercise the "give-up after `MAX_BIND_ATTEMPTS`"
/// branch without relying on the production backoff schedule.
///
/// Exposed as `pub` (not `#[cfg(test)]`) because integration tests live
/// in `crates/engine/tests/` — a separate crate per Cargo's test layout —
/// and can only access this crate's *public* surface. `#[cfg(test)]` only
/// activates inside the unit-test build of this crate, not in dependent
/// test crates. `#[doc(hidden)]` keeps the symbol out of rustdoc; the
/// `_for_test` suffix and this note discourage downstream use. Revisit if
/// listener internals get refactored into a shape where the test hook can
/// move under a feature gate.
#[doc(hidden)]
pub async fn bind_with_retry_for_test(addr: SocketAddr, max_attempts: u32) -> Option<TcpListener> {
	let cancel = CancellationToken::new();
	bind_with_retry(addr, &cancel, max_attempts).await
}
