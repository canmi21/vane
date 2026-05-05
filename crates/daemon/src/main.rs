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
#[cfg(feature = "wasm")]
mod wasm_loader;
mod watcher;

use std::net::SocketAddr;
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
use vane_engine::VerbosityState;
use vane_engine::factories::{FetchFactories, MiddlewareFactories};
use vane_engine::flow_graph::FlowGraph;
use vane_engine::flow_log_sink::{BroadcastSink, FanoutSink, default_sink_from_env};
use vane_engine::tracing_broadcast::BroadcastTracingLayer;
use vane_engine::{BindConfig, ListenerSet, SecurityConfig, SecurityState};

use crate::mgmt_handlers::MgmtState;
use crate::providers::MetadataProviders;
use crate::watcher::{arm_watcher_subscription, spawn_watcher_handler};

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

#[allow(clippy::too_many_lines)]
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

	// Daemon-wide TLS session ticketer — must follow
	// `install_default_provider` (the backend RNG fuels the initial
	// key) and precede any `FlowGraph::link` (which reads the ticketer
	// into each listener's `ServerConfig`). Failure here is fatal —
	// it implies the kernel CSPRNG is unavailable. See 08-tls.md
	// § _Session ticket rotation_.
	vane_engine::tls::install_default_ticketer().expect("install rustls session ticketer");
	vane_engine::metrics::install_recorder().expect("install metrics recorder");
	// Note: the system trust store is loaded lazily on first
	// `build_client_config` call (via `vane_engine::tls::native_roots`).
	// Eager warm-up was tried and rejected: under parallel boot (e.g.
	// nextest spawning many daemons) it serialises every process on the
	// macOS keychain queue even when the daemon won't use HTTPS
	// upstream. The lazy path's outcome is logged once per process
	// inside `native_roots`'s init.

	tracing::info!(config_dir = %args.config_dir.display(), "loading config");
	let loaded = vane_core::config::load(&args.config_dir)?;
	tracing::info!(
		rule_files = loaded.files.len(),
		bind_ipv4 = loaded.env.bind_ipv4,
		bind_ipv6 = loaded.env.bind_ipv6,
		"config loaded",
	);

	// Surface operator-tunable env vars not in the typed `Env` struct
	// (those that affect feature-gated subsystems and are read via
	// `OnceLock` at first invocation rather than at boot). Logging
	// here makes the active value visible in the startup log even
	// when no CGI request has fired yet. See
	// `spec/architecture/15-cgi.md` § _Concurrency cap_.
	let cgi_max_concurrent = std::env::var("VANE_CGI_MAX_CONCURRENT")
		.ok()
		.and_then(|s| s.parse::<usize>().ok())
		.filter(|n| *n > 0)
		.unwrap_or(100);
	tracing::info!(cgi_max_concurrent, "cgi concurrency cap resolved");

	// Boot-time WASM scan: must happen before the first compile so the
	// `MetadataProviders` knows which `<module>:<export>` plugin
	// references resolve. `loaded_wasm` stays `None` when the daemon
	// is built without `wasm`, when the dir is missing, or when every
	// load failed — in every case we fall through to the no-plugin
	// link path.
	#[cfg(feature = "wasm")]
	let loaded_wasm = wasm_loader::load_all(&loaded.env.wasm_dir).await;

	#[cfg(feature = "wasm")]
	let plugin_registry: Option<Arc<vane_engine::flow_graph::PluginRegistry>> =
		loaded_wasm.as_ref().map(|lw| Arc::clone(&lw.registry));
	#[cfg(not(feature = "wasm"))]
	let plugin_registry: Option<Arc<vane_engine::flow_graph::PluginRegistry>> = None;

	// Boot ref-check (must run *before* compile): walk the raw rule
	// JSON for every `<module>:<export>` plugin reference and refuse
	// to start when any of them is missing from `plugin_registry`.
	// Compile would otherwise fail with the first single "unknown
	// middleware" error; the curated list this produces gives the
	// operator the full fix list at once, and runs even when rule
	// shape would otherwise blow up downstream phase checks.
	let missing_plugins = collect_missing_plugin_refs(&loaded.files, plugin_registry.as_ref());
	if !missing_plugins.is_empty() {
		return Err(
			format!(
				"refusing to start: rules reference unloaded wasm plugins: [{}]; \
			 restart with the matching .wasm files present in {}",
				missing_plugins.join(", "),
				loaded.env.wasm_dir.display(),
			)
			.into(),
		);
	}

	let providers = match plugin_registry.as_ref() {
		#[cfg(feature = "wasm")]
		Some(reg) => MetadataProviders::with_plugins(Arc::clone(reg)),
		#[cfg(not(feature = "wasm"))]
		Some(_) => MetadataProviders::new(),
		None => MetadataProviders::new(),
	};
	let symbolic = compile(loaded.files, &providers, &providers)?;
	tracing::info!(
		nodes = symbolic.nodes.len(),
		entries = symbolic.entries.len(),
		middlewares = symbolic.middlewares.len(),
		fetches = symbolic.fetches.len(),
		"compiled symbolic flow graph",
	);

	// CRL cache: collected once across all listener client_auth + upstream
	// args.tls.crls sources, fetched synchronously at link time (30s per
	// source), and shared daemon-wide. Per
	// `spec/architecture/08-tls.md` § _CRL checking_, the cache key is
	// source identity (path / URL string) so refreshing CRL bytes does
	// not invalidate cached `Arc<ClientConfig>` / `Arc<ServerConfig>`.
	let crl_cache = init_crl_cache(&symbolic)?;

	// L1 security floor: validate env floors, build daemon-scoped state.
	let mut security_cfg_inner = SecurityConfig::new(&loaded.env)?;
	security_cfg_inner.crl_cache = crl_cache.clone();
	let security_cfg = Arc::new(security_cfg_inner);
	let security = Arc::new(SecurityState::new((*security_cfg).clone()));
	tracing::info!(
		header_timeout_secs = security_cfg.header_timeout.as_secs(),
		max_conn_per_ip = security_cfg.max_conn_per_ip,
		max_total_conns = security_cfg.max_total_conns,
		crl_cache = security_cfg.crl_cache.is_some(),
		"L1 security floor configured",
	);

	let mw_factories = Arc::new(build_middleware_factories());
	let fetch_factories = Arc::new(build_fetch_factories(security_cfg.crl_cache.clone()));
	let initial_graph = match plugin_registry.as_ref() {
		Some(reg) => FlowGraph::link_with_plugins(
			symbolic,
			&mw_factories,
			reg,
			&fetch_factories,
			Arc::clone(&security_cfg),
		)?,
		None => FlowGraph::link_with_security(
			symbolic,
			&mw_factories,
			&fetch_factories,
			Arc::clone(&security_cfg),
		)?,
	};
	let graph_swap: Arc<ArcSwap<FlowGraph>> = Arc::new(ArcSwap::new(initial_graph));
	tracing::info!("linked flow graph");

	// Compose the runtime flow-log sink. The default (`RingBufferSink`,
	// optionally an env-driven `FileSink`) is wrapped in a `FanoutSink`
	// alongside a `BroadcastSink` so the mgmt `tail_flow` verb has a
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

	let listeners = Arc::new(ListenerSet::from_security_and_bind_config(
		Arc::clone(&security),
		BindConfig::from(&loaded.env),
	));

	// Phase 1 of file-watcher startup: build the FSEvents subscription
	// BEFORE calling `listeners.start`. Once a listener is reachable on
	// its bound port the operator can drop a new rule file and rightly
	// expect it to take effect; if the watcher subscribed late, that
	// drop's fs event would fire in the gap and be lost (FSEvents on
	// macOS does not replay events that pre-date subscription). The
	// debouncer's mpsc channel is unbounded — events queued before the
	// handler task spawns just sit there until phase 2 drains them.
	// Init failure (typically permission-denied at the directory) is
	// logged and the daemon proceeds without auto-reload.
	let watcher_sub = match arm_watcher_subscription(args.config_dir.clone()) {
		Ok(s) => Some(s),
		Err(e) => {
			tracing::warn!(error = %e, "file watcher disabled — auto-reload unavailable");
			None
		}
	};

	listeners.start(Arc::clone(&graph_swap), Arc::clone(&verbosity), Arc::clone(&sink));
	tracing::info!(active = listeners.len(), "listeners started");

	// `shutdown_trigger` is shared by the boot health watchdog (fires
	// it on total bind failure), the mgmt `shutdown` verb, and the
	// `wait_for_shutdown_signal` select loop. Constructed once here and
	// cloned into each consumer.
	let shutdown_trigger = CancellationToken::new();

	// CRL background refresher: one tokio task per URL source, scheduled
	// off `nextUpdate − 1h`. File sources are not refreshed here — they
	// re-read on `FlowGraph` reload via `init_crl_cache` (the watcher
	// path).
	if let Some(cache) = &security_cfg.crl_cache {
		cache.spawn_refresher(&shutdown_trigger);
	}

	// Background cleanup for L1 security state: prunes zero-count
	// per-IP entries and stale log-dedup slots every 60 seconds.
	Arc::clone(&security).spawn_cleanup(shutdown_trigger.clone());

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
			loaded.env.boot_health_timeout_secs,
		);
	}

	// Phase 2 of file-watcher startup: spawn the handler task that
	// drains queued reload signals. Subscription was armed before
	// listeners.start (above) so any event landing in the bind window
	// is already queued in the unbounded mpsc; this task picks them
	// up immediately on first poll.
	let watcher_cancel = CancellationToken::new();
	let watcher_handle = match watcher_sub {
		Some(sub) => {
			let h = spawn_watcher_handler(
				sub,
				Arc::clone(&graph_swap),
				Arc::clone(&listeners),
				Arc::clone(&verbosity),
				Arc::clone(&sink),
				Arc::clone(&mw_factories),
				Arc::clone(&fetch_factories),
				Arc::clone(&security_cfg),
				plugin_registry.clone(),
				watcher_cancel.clone(),
			);
			tracing::info!("file watcher armed");
			Some(h)
		}
		None => None,
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
		security_cfg: Arc::clone(&security_cfg),
		shutdown_trigger: shutdown_trigger.clone(),
		#[cfg(feature = "wasm")]
		wasm_pool_stats: loaded_wasm
			.as_ref()
			.map(|lw| Arc::clone(&lw.runtime) as Arc<dyn vane_core::WasmPoolStats>),
		#[cfg(not(feature = "wasm"))]
		wasm_pool_stats: None,
		plugin_registry: plugin_registry.clone(),
	});
	let mgmt_cancel = CancellationToken::new();
	let mgmt_unix_handle = bind_mgmt_unix_server(Arc::clone(&mgmt_state), mgmt_cancel.clone()).await;
	let mgmt_http_handles = bind_mgmt_http_server(
		Arc::clone(&mgmt_state),
		mgmt_cancel.clone(),
		loaded.env.bind_ipv4,
		loaded.env.bind_ipv6,
	)
	.await?;

	wait_for_shutdown_signal(
		listeners,
		watcher_cancel,
		watcher_handle,
		mgmt_cancel,
		mgmt_unix_handle,
		mgmt_http_handles,
		shutdown_trigger,
		sigterm,
		sigint,
		Duration::from_secs(loaded.env.drain_timeout_secs.into()),
	)
	.await;
	Ok(())
}

fn init_tracing(tail_layer: BroadcastTracingLayer) {
	// `RUST_LOG` (env-filter) gates only the fmt-to-stderr layer —
	// the broadcast layer is intentionally unfiltered so that `vane
	// `tail log` shows every event the daemon emits regardless of how
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
	vane_engine::middleware::sni_peek::register(&mut mw);
	mw
}

/// Construct the daemon-wide CRL cache when the loaded ruleset names at
/// least one CRL source. The fetch is synchronous (block-in-place via
/// `ensure_loaded`) — `reject` policy sources whose first fetch fails
/// surface as a daemon-startup error, matching
/// `spec/architecture/08-tls.md` § _Failure handling_.
fn init_crl_cache(
	sym: &vane_core::SymbolicFlowGraph,
) -> Result<Option<Arc<vane_engine::tls::CrlCache>>, Box<dyn std::error::Error + Send + Sync>> {
	let listener_sources = vane_engine::tls::collect_listener_crl_sources(&sym.meta.listener_tls);
	let upstream_sources = vane_engine::tls::collect_upstream_crl_sources(sym);
	let sources =
		vane_engine::tls::dedupe_crl_sources(listener_sources.into_iter().chain(upstream_sources));
	if sources.is_empty() {
		tracing::debug!("no CRL sources in ruleset; skipping CRL cache init");
		return Ok(None);
	}
	tracing::info!(count = sources.len(), "loading CRL sources");
	let fetcher = vane_engine::tls::DefaultCrlFetcher::new_arc()?;
	let cache = vane_engine::tls::CrlCache::new(fetcher);
	cache.ensure_loaded(&sources)?;
	Ok(Some(cache))
}

fn build_fetch_factories(crl_cache: Option<Arc<vane_engine::tls::CrlCache>>) -> FetchFactories {
	let mut fetch = FetchFactories::new();
	vane_engine::fetch::l4_forward::register(&mut fetch);
	vane_engine::fetch::http_proxy::register(&mut fetch, crl_cache.clone());
	vane_engine::fetch::http_synthesize::register(&mut fetch);
	vane_engine::fetch::websocket_upgrade::register(&mut fetch, crl_cache);
	fetch
}

/// Bind the Unix mgmt socket. Returns the spawned task's `JoinHandle`
/// or `None` if bind failed — the daemon continues serving traffic in
/// that case.
async fn bind_mgmt_unix_server(
	mgmt_state: Arc<MgmtState>,
	cancel: CancellationToken,
) -> Option<tokio::task::JoinHandle<()>> {
	let socket = std::env::var("VANE_MGMT_UNIX").unwrap_or_else(|_| "/tmp/vaned.sock".to_string());
	match vane_mgmt::spawn_unix_server(std::path::Path::new(&socket), mgmt_state, cancel).await {
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
	}
}

/// Bind the HTTP-over-TCP mgmt transport per
/// `spec/architecture/10-management.md` § _Auth model_ and
/// `09-config.md` env-var section. Boot-validates the
/// `(VANE_MGMT_HTTP_PUBLIC, VANE_MGMT_HTTP_TOKEN)` pairing; bind
/// failures are fatal (the operator opted into HTTP transport, so a
/// missing port surfaces as a boot error rather than a silent
/// degradation).
///
/// Returns the per-bind task handles. Empty when the operator
/// disabled the transport via `VANE_MGMT_HTTP_PORT=`.
async fn bind_mgmt_http_server(
	mgmt_state: Arc<MgmtState>,
	cancel: CancellationToken,
	bind_ipv4: bool,
	bind_ipv6: bool,
) -> Result<Vec<tokio::task::JoinHandle<()>>, Box<dyn std::error::Error + Send + Sync>> {
	let Some(port) = parse_http_port()? else {
		tracing::info!("mgmt http transport disabled (VANE_MGMT_HTTP_PORT is empty)");
		return Ok(Vec::new());
	};
	let public = parse_truthy(std::env::var("VANE_MGMT_HTTP_PUBLIC").ok().as_deref());
	let token = std::env::var("VANE_MGMT_HTTP_TOKEN").ok().filter(|s| !s.is_empty());

	// Boot validation table — see spec/architecture/10-management.md
	// § _Auth model_. Public-without-token is a hard refuse so the
	// daemon never exposes plaintext mgmt to the public network.
	if public && token.is_none() {
		return Err(
			"VANE_MGMT_HTTP_PUBLIC=1 requires VANE_MGMT_HTTP_TOKEN to be set; \
			 refusing to bind plaintext management on the public network"
				.into(),
		);
	}
	if !public && token.is_none() {
		tracing::warn!(
			target: "mgmt",
			"management HTTP is unauthenticated on loopback; same-host users on this machine \
			 can issue management calls. Set VANE_MGMT_HTTP_TOKEN to enable bearer auth.",
		);
	}
	if !bind_ipv4 && !bind_ipv6 {
		return Err(
			"VANE_BIND_IPV4 and VANE_BIND_IPV6 are both disabled — no IP family available \
			 for management HTTP transport"
				.into(),
		);
	}

	let mut binds: Vec<SocketAddr> = Vec::new();
	if public {
		if bind_ipv4 {
			binds.push(format!("0.0.0.0:{port}").parse().expect("v4 wildcard"));
		}
		if bind_ipv6 {
			binds.push(format!("[::]:{port}").parse().expect("v6 wildcard"));
		}
	} else {
		if bind_ipv4 {
			binds.push(format!("127.0.0.1:{port}").parse().expect("v4 loopback"));
		}
		if bind_ipv6 {
			binds.push(format!("[::1]:{port}").parse().expect("v6 loopback"));
		}
	}

	let cfg = vane_mgmt::HttpServerConfig { binds, bearer_token: token.map(Into::into) };
	let handles = vane_mgmt::spawn_http_server(cfg, mgmt_state, cancel).await?;
	tracing::info!(count = handles.len(), port, public, "mgmt http server bound",);
	Ok(handles)
}

/// Parse `VANE_MGMT_HTTP_PORT`. `None` (env var unset) defaults to
/// 3333; an explicit empty string disables the transport entirely
/// (returns `Ok(None)`).
fn parse_http_port() -> Result<Option<u16>, Box<dyn std::error::Error + Send + Sync>> {
	match std::env::var("VANE_MGMT_HTTP_PORT").ok().as_deref() {
		None => Ok(Some(3333)),
		Some("") => Ok(None),
		Some(s) => match s.parse::<u16>() {
			Ok(p) => Ok(Some(p)),
			Err(e) => Err(format!("VANE_MGMT_HTTP_PORT: {e}").into()),
		},
	}
}

/// Boolean env-var parse used for `VANE_MGMT_HTTP_PUBLIC`. Truthy =
/// `1` / `true` / `yes` / `on` (case-insensitive). Anything else,
/// including unset / empty / `0` / `false` / `no` / `off`, is falsy.
fn parse_truthy(s: Option<&str>) -> bool {
	matches!(s.map(str::to_ascii_lowercase).as_deref(), Some("1" | "true" | "yes" | "on"),)
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
	timeout_secs: u32,
) {
	let timeout_secs = u64::from(timeout_secs);
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
	mgmt_unix_handle: Option<tokio::task::JoinHandle<()>>,
	mgmt_http_handles: Vec<tokio::task::JoinHandle<()>>,
	mgmt_shutdown_trigger: CancellationToken,
	mut sigterm: tokio::signal::unix::Signal,
	mut sigint: tokio::signal::unix::Signal,
	soft_drain: Duration,
) {
	let drain = tokio::select! {
		_ = sigterm.recv() => {
			tracing::info!(drain_secs = soft_drain.as_secs(), "SIGTERM received — soft drain");
			soft_drain
		}
		_ = sigint.recv() => {
			tracing::info!("SIGINT received — immediate shutdown");
			Duration::from_secs(0)
		}
		() = mgmt_shutdown_trigger.cancelled() => {
			tracing::info!(drain_secs = soft_drain.as_secs(), "mgmt shutdown verb received — soft drain");
			soft_drain
		}
	};
	watcher_cancel.cancel();
	mgmt_cancel.cancel();
	if let Some(h) = watcher_handle {
		let _ = h.await;
	}
	if let Some(h) = mgmt_unix_handle {
		let _ = h.await;
	}
	for h in mgmt_http_handles {
		let _ = h.await;
	}
	listeners.shutdown(drain).await;
	tracing::info!("vaned exited cleanly");
}

/// Walk every rule file's raw JSON for `<module>:<export>` strings
/// in `use:` slots, return the sorted, deduplicated set of plugin
/// names that the supplied `plugin_registry` cannot resolve.
///
/// Native middleware names are pure ASCII identifiers and never
/// contain `:`, so the colon split is unambiguous. The walk visits
/// every nested object / array so plugin references buried under
/// preset-specific shapes are caught regardless of where the rule
/// schema places them.
fn collect_missing_plugin_refs(
	files: &[vane_core::compile::RawRuleFile],
	plugin_registry: Option<&Arc<vane_engine::flow_graph::PluginRegistry>>,
) -> Vec<String> {
	use serde_json::Value;
	let mut refs: Vec<String> = Vec::new();
	for file in files {
		let v = serde_json::to_value(file).unwrap_or(Value::Null);
		walk_for_plugin_uses(&v, &mut refs);
	}
	refs.retain(|name| plugin_registry.is_none_or(|reg| reg.get(name).is_none()));
	refs.sort();
	refs.dedup();
	refs
}

fn walk_for_plugin_uses(v: &serde_json::Value, out: &mut Vec<String>) {
	match v {
		serde_json::Value::Object(map) => {
			if let Some(serde_json::Value::String(s)) = map.get("use")
				&& s.contains(':')
			{
				out.push(s.clone());
			}
			for child in map.values() {
				walk_for_plugin_uses(child, out);
			}
		}
		serde_json::Value::Array(arr) => {
			for child in arr {
				walk_for_plugin_uses(child, out);
			}
		}
		_ => {}
	}
}
