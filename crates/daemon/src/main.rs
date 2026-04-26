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
//! TODO(bind-failure-exit): when every listener's bind fails, the
//!   accept loop tasks exit individually but the daemon stays alive
//!   serving nothing. Operators see "all listener bind failures" only
//!   in `tracing` output. A future change should propagate
//!   "all listeners dead" upward and exit.
//! TODO(listener-set-diff): the file watcher refreshes the runtime
//!   `Arc<FlowGraph>` atomically, but `ListenerSet` does not add/remove
//!   sockets across reloads. Editing rules to introduce a new `listen`
//!   port still requires a daemon restart; the new port won't bind
//!   until then.

mod mgmt_handlers;
mod providers;
mod reload;
mod watcher;

use std::path::PathBuf;
use std::sync::Arc;
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
use vane_engine::flow_log_sink::default_sink_from_env;

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
	std::process::ExitCode::SUCCESS
}

async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
	let args = Args::parse();
	init_tracing();

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

	let sink: Arc<dyn FlowLogSink> = default_sink_from_env().await?;
	let verbosity = Arc::new(VerbosityState::new());

	let listeners = Arc::new(ListenerSet::new());
	listeners.start(Arc::clone(&graph_swap), Arc::clone(&verbosity), Arc::clone(&sink));
	tracing::info!(active = listeners.len(), "listeners started");

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
	// — the operator can fix the path and restart. `VANE_MGMT_HTTP_BIND`
	// is reserved for Stage 2 and currently ignored with a warn.
	let shutdown_trigger = CancellationToken::new();
	let mgmt_state = Arc::new(MgmtState {
		started_at: Instant::now(),
		graph_swap: Arc::clone(&graph_swap),
		listeners: Arc::clone(&listeners),
		mw_factories: Arc::clone(&mw_factories),
		fetch_factories: Arc::clone(&fetch_factories),
		config_dir: args.config_dir.clone(),
		verbosity: Arc::clone(&verbosity),
		log_sink: Arc::clone(&sink),
		shutdown_trigger: shutdown_trigger.clone(),
	});

	if std::env::var("VANE_MGMT_HTTP_BIND").is_ok() {
		tracing::warn!("VANE_MGMT_HTTP_BIND is reserved for Stage 2 — ignoring for now");
	}
	let mgmt_socket =
		std::env::var("VANE_MGMT_UNIX").unwrap_or_else(|_| "/tmp/vaned.sock".to_string());
	let mgmt_cancel = CancellationToken::new();
	let mgmt_handle = match vane_mgmt::spawn_unix_server(
		std::path::Path::new(&mgmt_socket),
		Arc::clone(&mgmt_state),
		mgmt_cancel.clone(),
	)
	.await
	{
		Ok(h) => {
			tracing::info!(socket = %mgmt_socket, "mgmt unix server bound");
			Some(h)
		}
		Err(e) => {
			tracing::warn!(socket = %mgmt_socket, error = %e, "mgmt unix server bind failed; daemon continues without mgmt");
			None
		}
	};

	wait_for_shutdown_signal(
		listeners,
		watcher_cancel,
		watcher_handle,
		mgmt_cancel,
		mgmt_handle,
		shutdown_trigger,
	)
	.await;
	Ok(())
}

fn init_tracing() {
	// `RUST_LOG` (env-filter) is the operator-facing knob. Default `info`
	// matches the `VANE_LOG_LEVEL` default surfaced by `Env`.
	let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
	tracing_subscriber::fmt().with_env_filter(filter).with_target(true).init();
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

async fn wait_for_shutdown_signal(
	listeners: Arc<ListenerSet>,
	watcher_cancel: CancellationToken,
	watcher_handle: Option<tokio::task::JoinHandle<()>>,
	mgmt_cancel: CancellationToken,
	mgmt_handle: Option<tokio::task::JoinHandle<()>>,
	mgmt_shutdown_trigger: CancellationToken,
) {
	let mut sigterm = signal(SignalKind::terminate()).expect("install SIGTERM handler");
	let mut sigint = signal(SignalKind::interrupt()).expect("install SIGINT handler");
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
