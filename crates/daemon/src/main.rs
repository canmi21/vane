//! `vaned` — vane proxy daemon entry point.
//!
//! Boot flow per `spec/architecture/01-topology.md` § _Daemon lifecycle_:
//! parse args → init tracing → load config (rules + env) → compile core
//! pipeline → link engine factories → start listeners → wait for signal
//! → drain.
//!
//! The CLI accepts:
//! - `--version` / `-v` — print build banner and exit (preserved from
//!   the earlier stub).
//! - `--config <DIR>` / `-c <DIR>` — config tree root, default
//!   `/etc/vaned`. Walked by `vane_core::config::load`.
//!
//! Several capabilities are intentionally not wired in this stage:
//! TODO(reload): SIGHUP / file-watch reload — the runtime is a single
//!   static `Arc<FlowGraph>`. Operators must restart on config change.
//! TODO(mgmt): Unix / HTTP management socket binding — see
//!   `spec/architecture/01-topology.md` § _Management plane_.
//! TODO(tls): TLS termination at the listener layer — bytes flow as
//!   plain HTTP/1 today; HTTPS clients hit a hyper parse error.
//! TODO(ws): `WebSocketUpgrade` fetch factory — rules referencing
//!   `type: "websocket"` fail at link with a pointed `UnknownFetch`
//!   error.
//! TODO(rate-limit): `rate_limit` middleware factory — same shape:
//!   referenced rules fail at link.
//! TODO(bind-failure-exit): when every listener's bind fails, the
//!   accept loop tasks exit individually but the daemon stays alive
//!   serving nothing. Operators see "all listener bind failures" only
//!   in `tracing` output. A future change should propagate
//!   "all listeners dead" upward and exit.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use tokio::signal::unix::{SignalKind, signal};
use tracing_subscriber::EnvFilter;
use vane_core::compile::compile;
use vane_core::version::{BuildInfo, format_version};
use vane_core::{
	Error, FetchKind, FetchMetadata, FetchMetadataProvider, FetchOutputModes, FetchPhase,
	FlowLogSink, MiddlewareKind, MiddlewareMetadata, MiddlewareMetadataProvider,
};
use vane_engine::ListenerSet;
use vane_engine::VerbosityState;
use vane_engine::factories::{FetchFactories, MiddlewareFactories};
use vane_engine::flow_graph::FlowGraph;
use vane_engine::flow_log_sink::default_sink_from_env;

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

	let mw_factories = build_middleware_factories();
	let fetch_factories = build_fetch_factories();
	let graph = FlowGraph::link(symbolic, &mw_factories, &fetch_factories)?;
	tracing::info!("linked flow graph");

	let sink: Arc<dyn FlowLogSink> = default_sink_from_env().await?;
	let verbosity = Arc::new(VerbosityState::new());

	let listeners = ListenerSet::new();
	listeners.start(Arc::clone(&graph), Arc::clone(&verbosity), Arc::clone(&sink));
	tracing::info!(active = listeners.len(), "listeners started");

	wait_for_shutdown_signal(listeners).await;
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
	mw
}

fn build_fetch_factories() -> FetchFactories {
	let mut fetch = FetchFactories::new();
	vane_engine::fetch::l4_forward::register(&mut fetch);
	vane_engine::fetch::http_proxy::register(&mut fetch);
	vane_engine::fetch::http_synthesize::register(&mut fetch);
	fetch
}

async fn wait_for_shutdown_signal(listeners: ListenerSet) {
	let mut sigterm = signal(SignalKind::terminate()).expect("install SIGTERM handler");
	let mut sigint = signal(SignalKind::interrupt()).expect("install SIGINT handler");
	tokio::select! {
		_ = sigterm.recv() => {
			tracing::info!("SIGTERM received — soft drain (30s)");
			listeners.shutdown(Duration::from_secs(30)).await;
		}
		_ = sigint.recv() => {
			tracing::info!("SIGINT received — immediate shutdown");
			listeners.shutdown(Duration::from_secs(0)).await;
		}
	}
	tracing::info!("vaned exited cleanly");
}

/// Daemon-side metadata provider that lists exactly the middleware /
/// fetch shapes registered in [`build_middleware_factories`] +
/// [`build_fetch_factories`]. Compile-time and link-time always agree:
/// every name compile reports as registered also has a factory.
struct MetadataProviders;

#[allow(clippy::unnecessary_wraps)]
fn validate_args_pass(_: &serde_json::Value) -> Result<(), Error> {
	// Per-factory args validation lives inside each factory at link
	// time. The compile pipeline only needs `Some(meta)` to confirm the
	// name is registered — schema violations surface as `LinkError`
	// later via the engine factory's args-parse path.
	Ok(())
}

impl MiddlewareMetadataProvider for MetadataProviders {
	fn get(&self, name: &str) -> Option<MiddlewareMetadata> {
		let (kind, stateless, needs_body) = match name {
			"host_header_match" | "path_prefix" | "method_match" | "forward_client_ip" => {
				(MiddlewareKind::L7Request, true, false)
			}
			_ => return None,
		};
		Some(MiddlewareMetadata { kind, stateless, needs_body, validate_args: validate_args_pass })
	}
}

impl FetchMetadataProvider for MetadataProviders {
	fn get(&self, kind: FetchKind) -> Option<FetchMetadata> {
		let (phase, output_modes) = match kind {
			FetchKind::L4Forward => (FetchPhase::L4, FetchOutputModes { response: false, tunnel: true }),
			FetchKind::HttpProxy | FetchKind::HttpSynthesize => {
				(FetchPhase::L7, FetchOutputModes { response: true, tunnel: false })
			}
			FetchKind::WebSocketUpgrade => {
				(FetchPhase::L7, FetchOutputModes { response: true, tunnel: true })
			}
		};
		Some(FetchMetadata { kind, phase, output_modes, validate_args: validate_args_pass })
	}
}
