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
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use arc_swap::ArcSwap;
use dashmap::DashMap;
use parking_lot::Mutex;
use tokio::net::{TcpListener, TcpSocket, TcpStream};
use tokio::sync::Mutex as AsyncMutex;
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;
use vane_core::{
	ConnContext, ConnId, FlowCtx, FlowLogSink, HttpVersion, L4Conn, NodeId, TlsInfo, TlsVersion,
	TrajectoryBuilder, Transport, config::Env,
};

use crate::executor::{ExecutorInput, execute};
use crate::flow_graph::FlowGraph;
use crate::peeked_stream::PeekedStream;
use crate::protocol_detect::classify;
use crate::verbosity::VerbosityState;
use vane_core::{MAX_PEEK_BYTES, PeekResult};

/// Per-connection budget for the listener-side peek prelude. The
/// prelude reads at most [`MAX_PEEK_BYTES`] before declaring the
/// prefix `Unknown`, so a hard wall-clock cap defends only against
/// pathological clients that establish TCP and then never write.
// TODO(s1-30): replace with `VANE_SEC_HEADER_TIMEOUT` once the L1
// security floor lands.
const PEEK_DEADLINE: Duration = Duration::from_secs(5);

const TCP_LISTEN_BACKLOG: u32 = 1024;

/// Operational knobs for the listener subsystem. All values have
/// spec-defined defaults (01-topology.md § _Bind_ / _Listener lifecycle_);
/// operators override via the `VANE_*` env vars documented in
/// `spec/architecture/09-config.md`.
#[derive(Clone, Debug)]
pub struct BindConfig {
	/// Bind-retry count per address (`VANE_BIND_MAX_ATTEMPTS`, default 10).
	pub max_bind_attempts: u32,
	/// Initial exponential-backoff delay between bind retries
	/// (`VANE_BIND_BACKOFF_INITIAL_MS`, default 100 ms).
	pub bind_backoff_initial: Duration,
	/// Cap for exponential-backoff delay
	/// (`VANE_BIND_BACKOFF_MAX_MS`, default 5 s).
	pub bind_backoff_max: Duration,
	/// Secondary grace window after `force_cancel` fires before the abort
	/// hammer drops (`VANE_FORCE_CANCEL_GRACE_SECS`, default 5 s).
	/// Applies to both SIGTERM drain and removed-listener reconcile.
	pub force_cancel_grace: Duration,
	/// Drain budget for in-flight connections when a listener is removed
	/// during reconcile (`VANE_DRAIN_TIMEOUT_SECS`, default 30 s).
	pub reconcile_drain_timeout: Duration,
}

impl Default for BindConfig {
	fn default() -> Self {
		Self {
			max_bind_attempts: 10,
			bind_backoff_initial: Duration::from_millis(100),
			bind_backoff_max: Duration::from_secs(5),
			force_cancel_grace: Duration::from_secs(5),
			reconcile_drain_timeout: Duration::from_secs(30),
		}
	}
}

impl From<&Env> for BindConfig {
	fn from(env: &Env) -> Self {
		Self {
			max_bind_attempts: env.bind_max_attempts,
			bind_backoff_initial: Duration::from_millis(env.bind_backoff_initial_ms.into()),
			bind_backoff_max: Duration::from_millis(env.bind_backoff_max_ms.into()),
			force_cancel_grace: Duration::from_secs(env.force_cancel_grace_secs.into()),
			reconcile_drain_timeout: Duration::from_secs(env.drain_timeout_secs.into()),
		}
	}
}

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
/// duplicate addresses (already-running keys are skipped with a warn).
pub struct ListenerSet {
	running: Mutex<HashMap<SocketAddr, ListenerHandle>>,
	/// Daemon-wide live-connection registry. Populated at accept time
	/// and cleaned up via [`ConnRegistration`] when the per-connection
	/// task ends. Read by the mgmt `list_connections` verb.
	connections: Arc<DashMap<ConnId, ConnEntry>>,
	bind_cfg: Arc<BindConfig>,
}

/// One in-flight connection's projection for the management plane.
/// Lives in [`ListenerSet::connections`] for the duration of the
/// per-connection task; the [`ConnRegistration`] guard removes it on
/// any exit path (success, panic, cancellation).
#[derive(Clone, Debug)]
pub struct ConnEntry {
	pub conn_id: ConnId,
	/// Local address of the listener that accepted this connection.
	pub listener_addr: SocketAddr,
	pub remote: SocketAddr,
	pub accepted_at: Instant,
}

/// RAII guard: removes `conn_id` from the daemon-wide connection
/// registry when dropped. One guard per spawned `handle_connection`
/// task — ensures the registry doesn't leak entries on panic /
/// cancellation, just like [`InFlightGuard`] for the counter.
struct ConnRegistration {
	registry: Arc<DashMap<ConnId, ConnEntry>>,
	conn_id: ConnId,
}

impl Drop for ConnRegistration {
	fn drop(&mut self) {
		self.registry.remove(&self.conn_id);
	}
}

struct ListenerHandle {
	accept_cancel: CancellationToken,
	force_cancel: CancellationToken,
	in_flight: Arc<AsyncMutex<JoinSet<()>>>,
	/// Live count of accepted-but-not-yet-completed connections on this
	/// listener. Bumped at spawn, decremented via RAII guard so panics
	/// and cancellations don't leak the counter. Surfaced through
	/// [`ListenerSet::in_flight_count`] for the mgmt `stats` /
	/// `list_connections` verbs.
	in_flight_count: Arc<AtomicUsize>,
	/// Flipped to `true` exactly once, by the accept-loop task, after
	/// its `bind_with_retry` returns a real `TcpListener`. Stays `false`
	/// when retries exhaust without success. Read by
	/// [`ListenerSet::bound_count`] so the daemon's boot health watchdog
	/// can distinguish "still trying" / "succeeded" / "gave up".
	bind_ready: Arc<AtomicBool>,
	join: JoinHandle<()>,
}

/// RAII guard: decrements the per-listener in-flight counter when
/// dropped. Construct one per spawned `handle_connection` call so the
/// counter survives panics, cancellations, and `?`-early-returns.
struct InFlightGuard(Arc<AtomicUsize>);

impl Drop for InFlightGuard {
	fn drop(&mut self) {
		self.0.fetch_sub(1, Ordering::Relaxed);
	}
}

impl ListenerSet {
	/// Create a [`ListenerSet`] with default [`BindConfig`] values.
	/// Production callers use [`Self::from_bind_config`] to apply
	/// operator-controlled env-var overrides.
	#[must_use]
	pub fn new() -> Self {
		Self::from_bind_config(BindConfig::default())
	}

	/// Create a [`ListenerSet`] from an explicit [`BindConfig`].
	/// Typically called as `ListenerSet::from_bind_config(BindConfig::from(&env))`.
	#[must_use]
	pub fn from_bind_config(cfg: BindConfig) -> Self {
		Self {
			running: Mutex::new(HashMap::new()),
			connections: Arc::new(DashMap::new()),
			bind_cfg: Arc::new(cfg),
		}
	}

	/// Snapshot the in-flight connection registry. Each entry is cloned
	/// from the shared [`DashMap`]; the snapshot is independent of the
	/// underlying registry once the call returns.
	#[must_use]
	pub fn list_connections(&self) -> Vec<ConnEntry> {
		self.connections.iter().map(|kv| kv.value().clone()).collect()
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
			let in_flight_count = Arc::new(AtomicUsize::new(0));
			let bind_ready = Arc::new(AtomicBool::new(false));

			let join = tokio::spawn(run_accept_loop(
				addr,
				Arc::clone(&graph),
				Arc::clone(&verbosity),
				Arc::clone(&log_sink),
				accept_cancel.clone(),
				force_cancel.clone(),
				Arc::clone(&in_flight),
				Arc::clone(&in_flight_count),
				Arc::clone(&bind_ready),
				Arc::clone(&self.connections),
				Arc::clone(&self.bind_cfg),
			));

			running.insert(
				addr,
				ListenerHandle {
					accept_cancel,
					force_cancel,
					in_flight,
					in_flight_count,
					bind_ready,
					join,
				},
			);
		}
	}

	/// Whether a listener is currently running for `addr`. Useful for tests
	/// that observe lifecycle transitions.
	///
	/// "Running" here means the registry holds a [`ListenerHandle`] for
	/// `addr` — including listeners that are still in `bind_with_retry`
	/// or that gave up after exhausting retries. To check whether the
	/// underlying socket is actually bound and accepting, use
	/// [`Self::is_bound`].
	#[must_use]
	pub fn is_running(&self, addr: &SocketAddr) -> bool {
		self.running.lock().contains_key(addr)
	}

	/// Whether the listener at `addr` has reached the bound state — the
	/// accept loop's `bind_with_retry` returned a real `TcpListener`.
	/// Distinct from [`Self::is_running`] (which just checks registry
	/// membership). Surfaced through the mgmt `stats` /
	/// `list_connections` verbs as the truthful `bound` field.
	#[must_use]
	pub fn is_bound(&self, addr: &SocketAddr) -> bool {
		self.running.lock().get(addr).is_some_and(|h| h.bind_ready.load(Ordering::Acquire))
	}

	/// Live count of in-flight connections accepted by the listener at
	/// `addr`. Returns `None` if no listener is currently bound there.
	///
	/// Surfaces through the management plane (`stats`,
	/// `list_connections`). The count is updated with `Ordering::Relaxed`
	/// because consumers want a recent value, not a synchronization
	/// guarantee against other memory.
	#[must_use]
	pub fn in_flight_count(&self, addr: &SocketAddr) -> Option<usize> {
		self.running.lock().get(addr).map(|h| h.in_flight_count.load(Ordering::Relaxed))
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

	/// Number of listener tasks whose `bind_with_retry` has succeeded —
	/// i.e. listeners that are actually accepting connections right now.
	/// The daemon's boot health watchdog uses this to detect the
	/// "everything failed to bind" case versus "some succeeded" or
	/// "still trying". `Ordering::Acquire` pairs with the `Release`
	/// store inside the accept loop so a reader that sees `true` also
	/// sees the bound socket's effects.
	#[must_use]
	pub fn bound_count(&self) -> usize {
		self.running.lock().values().filter(|h| h.bind_ready.load(Ordering::Acquire)).count()
	}

	/// Number of listeners managed regardless of bind state. Equal to
	/// the number of `entries` keys at boot or after the most recent
	/// reconcile. The boot health watchdog compares this against
	/// [`Self::bound_count`] to decide whether the boot completed.
	#[must_use]
	pub fn expected_count(&self) -> usize {
		self.running.lock().len()
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
			let ListenerHandle {
				accept_cancel: _,
				force_cancel,
				in_flight,
				in_flight_count: _,
				bind_ready: _,
				join,
			} = handle;

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
				let _ =
					tokio::time::timeout(self.bind_cfg.force_cancel_grace, drain_in_flight(&in_flight)).await;
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
				tokio::spawn(drain_handle_async(
					addr,
					handle,
					self.bind_cfg.force_cancel_grace,
					self.bind_cfg.reconcile_drain_timeout,
				));
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
			let in_flight_count = Arc::new(AtomicUsize::new(0));
			let bind_ready = Arc::new(AtomicBool::new(false));
			let join = tokio::spawn(run_accept_loop(
				addr,
				Arc::clone(&graph),
				Arc::clone(&verbosity),
				Arc::clone(&log_sink),
				accept_cancel.clone(),
				force_cancel.clone(),
				Arc::clone(&in_flight),
				Arc::clone(&in_flight_count),
				Arc::clone(&bind_ready),
				Arc::clone(&self.connections),
				Arc::clone(&self.bind_cfg),
			));
			running.insert(
				addr,
				ListenerHandle {
					accept_cancel,
					force_cancel,
					in_flight,
					in_flight_count,
					bind_ready,
					join,
				},
			);
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
async fn drain_handle_async(
	addr: SocketAddr,
	handle: ListenerHandle,
	force_cancel_grace: Duration,
	reconcile_drain_timeout: Duration,
) {
	let ListenerHandle {
		accept_cancel,
		force_cancel,
		in_flight,
		in_flight_count: _,
		bind_ready: _,
		join,
	} = handle;

	// Stop accepting new connections.
	accept_cancel.cancel();
	let _ = join.await;

	// Soft drain.
	if tokio::time::timeout(reconcile_drain_timeout, drain_in_flight(&in_flight)).await.is_ok() {
		tracing::info!(?addr, "reconcile drain complete");
		return;
	}

	tracing::warn!(?addr, "reconcile drain timed out; firing force_cancel for in-flight");
	force_cancel.cancel();
	let _ = tokio::time::timeout(force_cancel_grace, drain_in_flight(&in_flight)).await;
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

#[allow(clippy::too_many_arguments)]
async fn run_accept_loop(
	addr: SocketAddr,
	graph: Arc<ArcSwap<FlowGraph>>,
	verbosity: Arc<VerbosityState>,
	log_sink: Arc<dyn FlowLogSink>,
	accept_cancel: CancellationToken,
	force_cancel: CancellationToken,
	in_flight: Arc<AsyncMutex<JoinSet<()>>>,
	in_flight_count: Arc<AtomicUsize>,
	bind_ready: Arc<AtomicBool>,
	connections: Arc<DashMap<ConnId, ConnEntry>>,
	bind_cfg: Arc<BindConfig>,
) {
	let Some(listener) = bind_with_retry(
		addr,
		&accept_cancel,
		bind_cfg.max_bind_attempts,
		bind_cfg.bind_backoff_initial,
		bind_cfg.bind_backoff_max,
	)
	.await
	else {
		tracing::error!(
			?addr,
			attempts = bind_cfg.max_bind_attempts,
			"listener bind failed after exhausting retries — giving up on this address",
		);
		// `bind_ready` stays `false` so the daemon's boot health
		// watchdog observes the failed listener and can react.
		return;
	};
	bind_ready.store(true, Ordering::Release);

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
						let cancelled =
							backoff_sleep(bind_cfg.bind_backoff_initial, &accept_cancel).await;
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
				// Per-accept TLS lookup. `None` for cleartext listeners; on
				// hot reload the new graph's `listener_tls` is read for
				// every fresh accept (existing connections retain their
				// captured `Arc<FlowGraph>`).
				let tls_cfg = captured.listener_tls(&addr).cloned();
				// Bump the in-flight counter and hand the guard to the spawned
				// task so the matching decrement runs on any exit path
				// (success, panic, cancellation).
				in_flight_count.fetch_add(1, Ordering::Relaxed);
				let in_flight_guard = InFlightGuard(Arc::clone(&in_flight_count));
				let registry = Arc::clone(&connections);
				in_flight.lock().await.spawn(handle_connection(
					stream,
					remote,
					addr,
					entry,
					captured,
					tls_cfg,
					verbosity,
					log_sink,
					force,
					in_flight_guard,
					registry,
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
	tls_cfg: Option<Arc<rustls::ServerConfig>>,
	verbosity: Arc<VerbosityState>,
	log_sink: Arc<dyn FlowLogSink>,
	force_cancel: CancellationToken,
	// Held purely for its `Drop` impl — the in-flight counter
	// decrement runs on every exit path including panics and cancellation.
	_in_flight_guard: InFlightGuard,
	registry: Arc<DashMap<ConnId, ConnEntry>>,
) {
	let conn_id = next_conn_id();
	// Register before any further work, hold the deregister guard for
	// the rest of the function. `DashMap::insert` does not panic and
	// `ConnRegistration` construction is panic-free, so the registry
	// can never see a stranded entry — the guard's `Drop` always runs.
	let accepted_at = Instant::now();
	registry.insert(conn_id, ConnEntry { conn_id, listener_addr: local, remote, accepted_at });
	let _conn_registration = ConnRegistration { registry: Arc::clone(&registry), conn_id };
	let conn = Arc::new(ConnContext {
		id: conn_id,
		remote,
		local,
		transport: Transport::Tcp,
		entered_at: accepted_at,
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

	// Disable Nagle once, before either the peek phase or the TLS
	// handshake gets a chance to consume the socket. L4Forward used to
	// own this call, but the peek path erases the concrete TcpStream
	// behind a `PeekedStream` adapter, so the listener has to do it
	// while the type is still in scope.
	let _ = stream.set_nodelay(true);

	// On-demand peek gating: only run the prelude if some node walked
	// from `entry` references an L4Peek middleware. Listeners whose
	// graph is L4Peek-free stay on the zero-copy fast path.
	if !graph.needs_peek(entry) {
		dispatch_no_peek(stream, tls_cfg, &graph, entry, &conn, &mut ctx, remote).await;
		return;
	}

	let (peeked_buffer, peeked_stream, peek_result) =
		match tokio::time::timeout(PEEK_DEADLINE, run_peek_phase(stream)).await {
			Ok(Ok(triple)) => triple,
			Ok(Err(e)) => {
				tracing::debug!(
					error = %e,
					conn_id = %conn.id,
					?remote,
					"peek phase read error; dropping connection",
				);
				return;
			}
			Err(_) => {
				tracing::debug!(
					conn_id = %conn.id,
					?remote,
					timeout_ms = u64::try_from(PEEK_DEADLINE.as_millis()).unwrap_or(u64::MAX),
					"peek phase timeout; dropping connection",
				);
				return;
			}
		};

	// Pre-fill ConnContext.tls.sni from the parsed ClientHello so L4
	// middleware running before an `Upgrade` node can read it. `tls.alpn`
	// and `tls.version` are post-handshake values; they are populated in
	// the TLS termination path below (06-l4.md § _L4 → L7 upgrade_).
	if let Some(tls_hello) = peek_result.tls.as_ref()
		&& tls_hello.sni.is_some()
	{
		let mut guard = conn.tls.lock();
		let info = guard.get_or_insert_with(TlsInfo::default);
		info.sni.clone_from(&tls_hello.sni);
	}

	{
		let mut user = conn.user.lock();
		user.insert(peek_result);
	}

	let peeked = PeekedStream::new(peeked_buffer, peeked_stream);

	if let Some(tls_cfg) = tls_cfg {
		run_tls(peeked, tls_cfg, &graph, entry, &conn, &mut ctx, remote).await;
	} else {
		let result = execute(
			&graph,
			entry,
			ExecutorInput::L4(Box::new(L4Conn::Peeked(Box::new(peeked)))),
			&conn,
			&mut ctx,
		)
		.await;
		if let Err(e) = result {
			tracing::warn!(error = %e, conn_id = %conn.id, "connection ended with error");
		}
	}
}

/// Cleartext or TLS dispatch when the listener's `FlowGraph` does
/// not need the peek prelude. Mirrors the original two-branch shape
/// from before peek wiring.
async fn dispatch_no_peek(
	stream: TcpStream,
	tls_cfg: Option<Arc<rustls::ServerConfig>>,
	graph: &Arc<FlowGraph>,
	entry: NodeId,
	conn: &Arc<ConnContext>,
	ctx: &mut FlowCtx,
	remote: SocketAddr,
) {
	let Some(tls_cfg) = tls_cfg else {
		let result =
			execute(graph, entry, ExecutorInput::L4(Box::new(L4Conn::Tcp(stream))), conn, ctx).await;
		if let Err(e) = result {
			tracing::warn!(error = %e, conn_id = %conn.id, "connection ended with error");
		}
		return;
	};
	run_tls(stream, tls_cfg, graph, entry, conn, ctx, remote).await;
}

/// Drive the rustls server handshake with whatever underlying stream
/// the caller has — raw `TcpStream` for the no-peek path, or a
/// `PeekedStream<TcpStream>` for the post-peek path. Generic so the
/// rewind buffer is invisible to rustls: `LazyConfigAcceptor` reads
/// from offset zero in either case.
async fn run_tls<S>(
	stream: S,
	tls_cfg: Arc<rustls::ServerConfig>,
	graph: &Arc<FlowGraph>,
	entry: NodeId,
	conn: &Arc<ConnContext>,
	ctx: &mut FlowCtx,
	remote: SocketAddr,
) where
	S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
	let lazy = tokio_rustls::LazyConfigAcceptor::new(rustls::server::Acceptor::default(), stream);
	let start = match lazy.await {
		Ok(s) => s,
		Err(e) => {
			tracing::debug!(
				error = %e,
				conn_id = %conn.id,
				?remote,
				"tls clientHello read failed; dropping connection",
			);
			return;
		}
	};

	{
		let hello = start.client_hello();
		let sni = hello.server_name().map(str::to_ascii_lowercase);
		let mut guard = conn.tls.lock();
		let info = guard.get_or_insert_with(TlsInfo::default);
		info.sni = sni;
	}

	let tls_stream = match start.into_stream(tls_cfg).await {
		Ok(s) => s,
		Err(e) => {
			tracing::debug!(
				error = %e,
				conn_id = %conn.id,
				?remote,
				"tls handshake failed; dropping connection",
			);
			return;
		}
	};

	{
		let (_io, server_conn) = tls_stream.get_ref();
		let alpn = server_conn.alpn_protocol().map(<[u8]>::to_vec);
		match alpn.as_deref() {
			Some(b"h2") => {
				let _ = conn.http_version.set(HttpVersion::Http2);
			}
			Some(b"http/1.1") => {
				let _ = conn.http_version.set(HttpVersion::Http1_1);
			}
			_ => {}
		}
		let mut guard = conn.tls.lock();
		let info = guard.get_or_insert_with(TlsInfo::default);
		info.alpn = alpn;
		info.version = server_conn.protocol_version().and_then(|v| match v {
			rustls::ProtocolVersion::TLSv1_2 => Some(TlsVersion::Tls12),
			rustls::ProtocolVersion::TLSv1_3 => Some(TlsVersion::Tls13),
			_ => None,
		});
	}

	let result = execute(
		graph,
		entry,
		ExecutorInput::L4(Box::new(L4Conn::Tls(Box::new(tls_stream)))),
		conn,
		ctx,
	)
	.await;
	if let Err(e) = result {
		tracing::warn!(error = %e, conn_id = %conn.id, "connection ended with error");
	}
}

/// Read up to [`MAX_PEEK_BYTES`] from `stream`, calling
/// [`classify`] after every read until a detector commits or the
/// buffer fills. Returns the accumulated buffer (as
/// [`bytes::Bytes`] for the [`PeekedStream`] rewind side), the
/// original `TcpStream` (so the caller can keep wrapping it), and
/// the structured [`PeekResult`].
async fn run_peek_phase(
	mut stream: TcpStream,
) -> std::io::Result<(bytes::Bytes, TcpStream, PeekResult)> {
	use tokio::io::AsyncReadExt;

	let mut buf = Vec::with_capacity(MAX_PEEK_BYTES);
	loop {
		let result = classify(&buf);
		if result.detected.is_some() {
			return Ok((bytes::Bytes::from(buf), stream, result));
		}
		if buf.len() >= MAX_PEEK_BYTES {
			// Buffer full and no detector committed — declare Unknown.
			let final_result = PeekResult {
				buffer: bytes::Bytes::copy_from_slice(&buf),
				detected: Some(vane_core::DetectedProtocol::Unknown),
				tls: None,
			};
			return Ok((bytes::Bytes::from(buf), stream, final_result));
		}
		// Read at least one more byte. `read_buf` would let us reuse
		// the spare capacity; manually growing keeps the buffer-as-
		// `Bytes::from` round-trip cheap.
		let read_at = buf.len();
		buf.resize(buf.capacity().min(MAX_PEEK_BYTES).max(buf.len() + 1), 0);
		match stream.read(&mut buf[read_at..]).await {
			Ok(0) => {
				// Peer EOF — classify whatever we have.
				buf.truncate(read_at);
				let final_result = PeekResult {
					buffer: bytes::Bytes::copy_from_slice(&buf),
					detected: Some(vane_core::DetectedProtocol::Unknown),
					tls: None,
				};
				return Ok((bytes::Bytes::from(buf), stream, final_result));
			}
			Ok(n) => buf.truncate(read_at + n),
			Err(e) => return Err(e),
		}
	}
}

/// Bind-with-retry per 01-topology.md § _Bind_:
/// - `SO_REUSEADDR` on (best-effort).
/// - Exponential backoff from `backoff_initial` up to `backoff_max`.
/// - Up to `max_attempts` tries.
/// - Honors `cancel`: if cancellation fires during a backoff window the
///   function aborts and returns `None`.
///
/// `max_attempts` is parametric so tests can drive the give-up branch
/// without burning real backoff time.
async fn bind_with_retry(
	addr: SocketAddr,
	cancel: &CancellationToken,
	max_attempts: u32,
	backoff_initial: Duration,
	backoff_max: Duration,
) -> Option<TcpListener> {
	let mut delay = backoff_initial;
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
				delay = (delay * 2).min(backoff_max);
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
		delay = (delay * 2).min(backoff_max);
	}
	None
}

/// Test-only entry point: drive `bind_with_retry` with a custom attempt
/// cap and default backoff timings. Tests use this to exercise the
/// "give-up after N attempts" branch without burning real backoff time.
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
	let cfg = BindConfig::default();
	bind_with_retry(addr, &cancel, max_attempts, cfg.bind_backoff_initial, cfg.bind_backoff_max).await
}
