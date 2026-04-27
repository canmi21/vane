//! `vaned` — vane proxy daemon entry point.
//!
//! Boot flow per `spec/architecture/01-topology.md` § _Daemon lifecycle_:
//! parse args → init tracing → load config (rules + env) → compile core
//! pipeline → link engine factories → wrap into `ArcSwap` → start
//! listeners → spawn file watcher (best-effort) → wait for signal →
//! cancel watcher → drain listeners.
//!
//! The CLI accepts:
//! - `--version` / `-v` — print build banner and exit (preserved from
//!   the earlier stub).
//! - `--config <DIR>` / `-c <DIR>` — config tree root, default
//!   `/etc/vaned`. Walked by `vane_core::config::load`.
//!
//! Several capabilities are intentionally not wired in this stage:
//! TODO(listener-set-diff): the file watcher refreshes the runtime
//!   `Arc<FlowGraph>` atomically, but `ListenerSet` does not add/remove
//!   sockets across reloads. Editing rules to introduce a new `listen`
//!   port still requires a daemon restart; the new port won't bind
//!   until then.
//!
//! The boot health watchdog (`spawn_boot_health_watchdog`) covers the
//! "all listeners failed to bind" case: on a configurable timeout
//! (`VANE_BOOT_HEALTH_TIMEOUT_SECS`, default 60s) with zero successful
//! binds it fires the shutdown trigger and sets [`BOOT_HEALTH_EXIT`]
//! so `main` returns a non-zero exit code. Partial bind failure stays
//! a warn — the daemon serves whatever bound, and operators can read
//! per-listener status via `vane stats`.

mod mgmt_handlers;
mod providers;
mod reload;
mod watcher;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use clap::Parser;
use tokio::signal::unix::{SignalKind, signal};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;
use vane_core::FlowLogSink;
use vane_core::compile::compile;
use vane_core::version::{BuildInfo, format_version};
use vane_engine::ListenerSet;
use vane_engine::VerbosityState;
use vane_engine::factories::{FetchFactories, MiddlewareFactories};
use vane_engine::flow_graph::FlowGraph;
use vane_engine::flow_log_sink::{BroadcastSink, FanoutSink, default_sink_from_env};
use vane_engine::tracing_broadcast::BroadcastTracingLayer;

use crate::mgmt_handlers::MgmtState;
use crate::providers::MetadataProviders;
use crate::watcher::spawn_watcher;

const FEATURES: &[&str] = &[
	#[cfg(feature = "aws-lc-rs")]
	"aws-lc-rs",
	#[cfg(feature = "ring")]
	"ring",
	#[cfg(feature = "h3")]
	"h3",
	#[cfg(feature = "cgi")]
	"cgi",
	#[cfg(feature = "wasm")]
	"wasm",
];

const PROTOCOLS: &[&str] = &[
	"tcp",
	"udp",
	#[cfg(feature = "h3")]
	"quic",
	"h1",
	"h2",
	#[cfg(feature = "h3")]
	"h3",
	"ws",
	#[cfg(feature = "cgi")]
	"cgi",
];

const BUILD_INFO: BuildInfo = BuildInfo {
	version: env!("CARGO_PKG_VERSION"),
	commit: env!("VANE_COMMIT"),
	build_date: env!("VANE_BUILD_DATE"),
	rustc: env!("VANE_RUSTC"),
	cargo: env!("VANE_CARGO"),
	features: FEATURES,
	protocols: PROTOCOLS,
};

#[derive(Parser, Debug)]
#[command(name = "vaned", about = "vane proxy daemon", disable_version_flag = true)]
struct Args {
	/// Path to the config directory (must contain a `rules/`
	/// sub-directory; optionally a `.env` for `VANE_*` overrides).
	#[arg(short = 'c', long = "config", default_value = "/etc/vaned")]
	config_dir: PathBuf,
}

/// Set by [`spawn_boot_health_watchdog`] when total bind failure forces
/// a shutdown. `main` reads this after `run()` returns so the process
/// exits with a non-zero code — supervisors then restart cleanly
/// instead of leaving an empty daemon up.
static BOOT_HEALTH_EXIT: AtomicBool = AtomicBool::new(false);

/// Default budget for every expected listener to flip its `bind_ready`
/// flag. Overridable via `VANE_BOOT_HEALTH_TIMEOUT_SECS` for tests and
/// for operators on slow networks where a temporarily occupied port
/// is expected to free up. Total bind retry budget per listener is
/// already capped by `MAX_BIND_ATTEMPTS` × `BIND_BACKOFF_MAX` inside
/// the engine.
const BOOT_HEALTH_TIMEOUT_DEFAULT_SECS: u64 = 60;

#[tokio::main]
async fn main() -> std::process::ExitCode {
	// Pre-clap fast path: `--version` / `-v` prints the build banner and
	// exits before any config loading kicks in. Operators on a fresh box
	// without `/etc/vaned` should still be able to verify the build.
	let raw: Vec<String> = std::env::args().collect();
	if raw.iter().any(|a| a == "--version" || a == "-v") {
		print!("{}", format_version(&BUILD_INFO));
		return std::process::ExitCode::SUCCESS;
	}

	if let Err(e) = run().await {
		eprintln!("vaned: {e}");
		return std::process::ExitCode::FAILURE;
	}
	if BOOT_HEALTH_EXIT.load(Ordering::Acquire) {
		return std::process::ExitCode::FAILURE;
	}
	std::process::ExitCode::SUCCESS
}

async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
	let args = Args::parse();
	// Construct the broadcast tracing layer first so the subscriber
	// stack composes it alongside the stderr fmt layer. The layer
	// itself is Clone (cheap — wraps a broadcast::Sender); we hand one
	// clone to the subscriber and keep the original for `MgmtState`.
	let tracing_broadcast = BroadcastTracingLayer::new();
	init_tracing(tracing_broadcast.clone());

	// Install rustls's process-wide default crypto provider before any
	// `ServerConfig::builder()` runs in `FlowGraph::link`. The selection
	// (aws-lc-rs vs ring) is fixed at compile time by the engine's
	// crypto-backend feature; see 16-crate-layout.md § _Crypto backend_.
	vane_engine::crypto::install_default_provider();

	tracing::info!(config_dir = %args.config_dir.display(), "loading config");
	let loaded = vane_core::config::load(&args.config_dir)?;
	tracing::info!(
		rule_files = loaded.files.len(),
		bind_ipv4 = loaded.env.bind_ipv4,
		bind_ipv6 = loaded.env.bind_ipv6,
		"config loaded",
	);

	let providers = MetadataProviders;
	let symbolic = compile(loaded.files, &providers, &providers)?;
	tracing::info!(
		nodes = symbolic.nodes.len(),
		entries = symbolic.entries.len(),
		middlewares = symbolic.middlewares.len(),
		fetches = symbolic.fetches.len(),
		"compiled symbolic flow graph",
	);

	let mw_factories = Arc::new(build_middleware_factories());
	let fetch_factories = Arc::new(build_fetch_factories());
	let initial_graph = FlowGraph::link(symbolic, &mw_factories, &fetch_factories)?;
	let graph_swap: Arc<ArcSwap<FlowGraph>> = Arc::new(ArcSwap::new(initial_graph));
	tracing::info!("linked flow graph");

	// Compose the runtime flow-log sink. The default (`RingBufferSink`,
	// optionally an env-driven `FileSink`) is wrapped in a `FanoutSink`
	// alongside a `BroadcastSink` so the mgmt `tail_flow_log` verb has a
	// live event source. The `BroadcastSink` is held separately on
	// `MgmtState` so handlers can call `subscribe()` directly.
	let default_sink = default_sink_from_env().await?;
	let broadcast_sink = Arc::new(BroadcastSink::new());
	let sink: Arc<dyn FlowLogSink> = Arc::new(FanoutSink::new(vec![
		default_sink,
		Arc::clone(&broadcast_sink) as Arc<dyn FlowLogSink>,
	]));
	let verbosity = Arc::new(VerbosityState::new());

	// Install POSIX shutdown-signal streams BEFORE any listener starts.
	// `tokio::signal::unix::signal()` registers the kernel-level handler
	// eagerly; from this point on SIGTERM / SIGINT are queued onto the
	// streams instead of taking their default termination disposition.
	// Wiring this earlier closes a startup race where a SIGINT delivered
	// between `listeners.start()` (port becomes reachable, which is the
	// readiness signal supervisors use) and the previous handler-install
	// site inside `wait_for_shutdown_signal` would kill the daemon
	// outright. The streams are awaited at the end of `main` via
	// [`wait_for_shutdown_signal`].
	let sigterm = signal(SignalKind::terminate()).expect("install SIGTERM handler");
	let sigint = signal(SignalKind::interrupt()).expect("install SIGINT handler");

	let listeners = Arc::new(ListenerSet::new());
	listeners.start(Arc::clone(&graph_swap), Arc::clone(&verbosity), Arc::clone(&sink));
	tracing::info!(active = listeners.len(), "listeners started");

	// `shutdown_trigger` is shared by the boot health watchdog (fires
	// it on total bind failure), the mgmt `shutdown` verb, and the
	// `wait_for_shutdown_signal` select loop. Constructed once here and
	// cloned into each consumer.
	let shutdown_trigger = CancellationToken::new();

	// Boot health watchdog: detect "every listener failed to bind"
	// within a bounded budget and force shutdown with a non-zero exit
	// code. Partial failures stay warn-only.
	let expected_listener_count = listeners.expected_count();
	if expected_listener_count == 0 {
		tracing::warn!("graph has no listener entries; daemon will serve nothing");
	} else {
		spawn_boot_health_watchdog(
			Arc::clone(&listeners),
			shutdown_trigger.clone(),
			expected_listener_count,
		);
	}

	// File watcher: best-effort. Init failure is logged and the daemon
	// continues without auto-reload — operators relying on watcher-driven
	// reload have to fix the underlying problem and restart.
	let watcher_cancel = CancellationToken::new();
	let watcher_handle = match spawn_watcher(
		args.config_dir.clone(),
		Arc::clone(&graph_swap),
		Arc::clone(&listeners),
		Arc::clone(&verbosity),
		Arc::clone(&sink),
		Arc::clone(&mw_factories),
		Arc::clone(&fetch_factories),
		watcher_cancel.clone(),
	) {
		Ok(h) => {
			tracing::info!("file watcher armed");
			Some(h)
		}
		Err(e) => {
			tracing::warn!(error = %e, "file watcher disabled — auto-reload unavailable");
			None
		}
	};

	// Management plane: bind the Unix mgmt socket and dispatch verbs to
	// `MgmtState`. Bind failures (e.g. directory missing, perms denied)
	// are logged and the daemon continues serving traffic without mgmt
	// — the operator can fix the path and restart.
	let mgmt_state = Arc::new(MgmtState {
		started_at: Instant::now(),
		graph_swap: Arc::clone(&graph_swap),
		listeners: Arc::clone(&listeners),
		mw_factories: Arc::clone(&mw_factories),
		fetch_factories: Arc::clone(&fetch_factories),
		config_dir: args.config_dir.clone(),
		verbosity: Arc::clone(&verbosity),
		log_sink: Arc::clone(&sink),
		broadcast: Arc::clone(&broadcast_sink),
		tracing_broadcast,
		shutdown_trigger: shutdown_trigger.clone(),
	});
	let (mgmt_cancel, mgmt_handle) = bind_mgmt_server(Arc::clone(&mgmt_state)).await;

	wait_for_shutdown_signal(
		listeners,
		watcher_cancel,
		watcher_handle,
		mgmt_cancel,
		mgmt_handle,
		shutdown_trigger,
		sigterm,
		sigint,
	)
	.await;
	Ok(())
}

fn init_tracing(tail_layer: BroadcastTracingLayer) {
	// `RUST_LOG` (env-filter) gates only the fmt-to-stderr layer —
	// the broadcast layer is intentionally unfiltered so that `vane
	// tail-log` shows every event the daemon emits regardless of how
	// noisy the operator's terminal is configured to be. Operators
	// who want to thin the stream client-side can pipe to `jq`.
	//
	// Default `info` for the fmt layer matches the `VANE_LOG_LEVEL`
	// default surfaced by `Env`.
	use tracing_subscriber::Layer;
	use tracing_subscriber::layer::SubscriberExt;
	use tracing_subscriber::util::SubscriberInitExt;
	let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
	let fmt_layer = tracing_subscriber::fmt::layer().with_target(true).with_filter(filter);
	tracing_subscriber::registry().with(fmt_layer).with(tail_layer).init();
}

fn build_middleware_factories() -> MiddlewareFactories {
	let mut mw = MiddlewareFactories::new();
	vane_engine::middleware::host_header_match::register(&mut mw);
	vane_engine::middleware::path_prefix::register(&mut mw);
	vane_engine::middleware::method_match::register(&mut mw);
	vane_engine::middleware::forward_client_ip::register(&mut mw);
	vane_engine::middleware::rate_limit::register(&mut mw);
	mw
}

fn build_fetch_factories() -> FetchFactories {
	let mut fetch = FetchFactories::new();
	vane_engine::fetch::l4_forward::register(&mut fetch);
	vane_engine::fetch::http_proxy::register(&mut fetch);
	vane_engine::fetch::http_synthesize::register(&mut fetch);
	vane_engine::fetch::websocket_upgrade::register(&mut fetch);
	fetch
}

/// Bind the Unix mgmt socket. Returns the cancel token feeding the
/// server task and the task's `JoinHandle` (or `None` if the bind
/// failed — the daemon continues serving traffic in that case).
/// `VANE_MGMT_HTTP_BIND` is reserved for Stage 2 and currently logged
/// at warn before being ignored.
async fn bind_mgmt_server(
	mgmt_state: Arc<MgmtState>,
) -> (CancellationToken, Option<tokio::task::JoinHandle<()>>) {
	if std::env::var("VANE_MGMT_HTTP_BIND").is_ok() {
		tracing::warn!("VANE_MGMT_HTTP_BIND is reserved for Stage 2 — ignoring for now");
	}
	let socket = std::env::var("VANE_MGMT_UNIX").unwrap_or_else(|_| "/tmp/vaned.sock".to_string());
	let cancel = CancellationToken::new();
	let handle =
		match vane_mgmt::spawn_unix_server(std::path::Path::new(&socket), mgmt_state, cancel.clone())
			.await
		{
			Ok(h) => {
				tracing::info!(socket = %socket, "mgmt unix server bound");
				Some(h)
			}
			Err(e) => {
				tracing::warn!(
					socket = %socket,
					error = %e,
					"mgmt unix server bind failed; daemon continues without mgmt",
				);
				None
			}
		};
	(cancel, handle)
}

/// Boot-time health check. Spawns a background task that polls
/// `bound_count` against `expected` once per second until either every
/// listener has bound or the configured budget expires.
///
/// Outcomes after timeout:
/// - **Zero bound** (every listener gave up): set [`BOOT_HEALTH_EXIT`]
///   and fire `shutdown_trigger`. The shutdown drains through the
///   normal mgmt-shutdown path and `main` returns
///   [`std::process::ExitCode::FAILURE`] so supervisors restart.
/// - **Partial bind** (some succeeded, some failed): warn-log and
///   leave the daemon running. Per-listener bound state is observable
///   via `vane stats`.
///
/// The 1Hz poll is generous compared to the default 60s budget; a
/// notification primitive would have to be wired into every status-
/// transition site (success path, retry-exhaust path) and the bound on
/// the race window would still be the polling period. Polling reads
/// the truth regardless of which path got us there.
fn spawn_boot_health_watchdog(
	listeners: Arc<ListenerSet>,
	shutdown_trigger: CancellationToken,
	expected: usize,
) {
	let timeout_secs = std::env::var("VANE_BOOT_HEALTH_TIMEOUT_SECS")
		.ok()
		.and_then(|s| s.parse::<u64>().ok())
		.unwrap_or(BOOT_HEALTH_TIMEOUT_DEFAULT_SECS);
	tokio::spawn(async move {
		let deadline = Instant::now() + Duration::from_secs(timeout_secs);
		loop {
			let bound = listeners.bound_count();
			if bound == expected {
				tracing::info!(bound, expected, "all listeners bound successfully");
				return;
			}
			if Instant::now() >= deadline {
				if bound == 0 {
					tracing::error!(
						expected,
						timeout_secs,
						"all listeners failed to bind within boot health timeout — daemon exiting"
					);
					BOOT_HEALTH_EXIT.store(true, Ordering::Release);
					shutdown_trigger.cancel();
				} else {
					tracing::warn!(
						bound,
						expected,
						timeout_secs,
						"boot health timeout reached; daemon continues with partial coverage",
					);
				}
				return;
			}
			tokio::time::sleep(Duration::from_secs(1)).await;
		}
	});
}

#[allow(clippy::too_many_arguments)]
async fn wait_for_shutdown_signal(
	listeners: Arc<ListenerSet>,
	watcher_cancel: CancellationToken,
	watcher_handle: Option<tokio::task::JoinHandle<()>>,
	mgmt_cancel: CancellationToken,
	mgmt_handle: Option<tokio::task::JoinHandle<()>>,
	mgmt_shutdown_trigger: CancellationToken,
	mut sigterm: tokio::signal::unix::Signal,
	mut sigint: tokio::signal::unix::Signal,
) {
	let drain = tokio::select! {
		_ = sigterm.recv() => {
			tracing::info!("SIGTERM received — soft drain (30s)");
			Duration::from_secs(30)
		}
		_ = sigint.recv() => {
			tracing::info!("SIGINT received — immediate shutdown");
			Duration::from_secs(0)
		}
		() = mgmt_shutdown_trigger.cancelled() => {
			tracing::info!("mgmt shutdown verb received — soft drain (30s)");
			Duration::from_secs(30)
		}
	};
	watcher_cancel.cancel();
	mgmt_cancel.cancel();
	if let Some(h) = watcher_handle {
		let _ = h.await;
	}
	if let Some(h) = mgmt_handle {
		let _ = h.await;
	}
	listeners.shutdown(drain).await;
	tracing::info!("vaned exited cleanly");
}
