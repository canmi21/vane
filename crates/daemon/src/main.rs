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
//! TODO(mgmt): Unix / HTTP management socket binding — see
//!   `spec/architecture/01-topology.md` § _Management plane_.
//! TODO(tls): TLS termination at the listener layer — bytes flow as
//!   plain HTTP/1 today; HTTPS clients hit a hyper parse error.
//! TODO(ws): `WebSocketUpgrade` fetch factory — rules referencing
//!   `type: "websocket"` fail at link with a pointed `UnknownFetch`
//!   error.
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

mod providers;
mod reload;
mod watcher;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

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

	wait_for_shutdown_signal(listeners, watcher_cancel, watcher_handle).await;
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
	fetch
}

async fn wait_for_shutdown_signal(
	listeners: Arc<ListenerSet>,
	watcher_cancel: CancellationToken,
	watcher_handle: Option<tokio::task::JoinHandle<()>>,
) {
	let mut sigterm = signal(SignalKind::terminate()).expect("install SIGTERM handler");
	let mut sigint = signal(SignalKind::interrupt()).expect("install SIGINT handler");
	tokio::select! {
		_ = sigterm.recv() => {
			tracing::info!("SIGTERM received — soft drain (30s)");
			watcher_cancel.cancel();
			if let Some(h) = watcher_handle {
				let _ = h.await;
			}
			listeners.shutdown(Duration::from_secs(30)).await;
		}
		_ = sigint.recv() => {
			tracing::info!("SIGINT received — immediate shutdown");
			watcher_cancel.cancel();
			if let Some(h) = watcher_handle {
				let _ = h.await;
			}
			listeners.shutdown(Duration::from_secs(0)).await;
		}
	}
	tracing::info!("vaned exited cleanly");
}
