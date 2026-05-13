//! `vaned` — vane proxy daemon entry point.
//!
//! Boot flow per `spec/topology.md` § _Daemon lifecycle_:
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
//! binds it fires the shutdown trigger and sets a `BOOT_HEALTH_EXIT`
//! flag so `main` returns a non-zero exit code. Partial bind failure stays
//! a warn — the daemon serves whatever bound, and operators can read
//! per-listener status via `vane stats`.

#[cfg(feature = "acme")]
mod acme_boot;
mod boot;
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
use tokio_util::sync::CancellationToken;
use tracing_broadcast::BroadcastTracingLayer;
use tracing_subscriber::EnvFilter;
use vane_banner::print_banner;
use vane_core::compile::compile;
use vane_core::version::BuildInfo;
use vane_engine::VerbosityState;
use vane_engine::factories::{FetchFactories, MiddlewareFactories};
use vane_engine::flow_graph::FlowGraph;
use vane_engine::{BindConfig, ListenerSet};

use crate::mgmt_handlers::MgmtState;
use crate::watcher::arm_watcher_subscription;

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
	/// `VANE_CONFIG_DIR` is honored when the flag is omitted, matching
	/// `spec/crates/core.md`.
	#[arg(short = 'c', long = "config", env = "VANE_CONFIG_DIR", default_value = "/etc/vaned")]
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
		print_banner(&BUILD_INFO);
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

#[allow(
	clippy::too_many_lines,
	reason = "daemon boot roadmap: ~12 named phase calls + their per-phase wiring (ReloadCtx, listener spawn, ACME, mgmt plane). Every line is sequenced orchestration; further extraction would hide the boot ordering across files without compressing intrinsic complexity"
)]
async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
	let args = Args::parse();

	// Load config + .env BEFORE initialising tracing so the file's
	// `VANE_LOG_LEVEL` is honored. dotenvy never overrides pre-existing
	// OS env, so `RUST_LOG` from the operator's shell still wins.
	let loaded = vane_core::config::load(&args.config_dir)?;

	// Install the drop counter on the tracing broadcast layer so an
	// operator running `vane stats` can see how many tracing frames
	// have been emitted while no `tail_log` subscriber was attached.
	let tracing_broadcast = BroadcastTracingLayer::with_capacity_and_drop_hook(
		tracing_broadcast::DEFAULT_BROADCAST_CAP,
		std::sync::Arc::new(|| {
			metrics::counter!("vane.trace.broadcast_dropped", "reason" => "no_subscribers").increment(1);
		}),
	);
	init_tracing(tracing_broadcast.clone(), &loaded.env.log_level);

	tracing::info!(config_dir = %args.config_dir.display(), "loading config");
	tracing::info!(
		rule_files = loaded.files.len(),
		bind_ipv4 = loaded.env.bind_ipv4,
		bind_ipv6 = loaded.env.bind_ipv6,
		"config loaded",
	);

	boot::install_global_runtime();
	boot::log_cgi_concurrency_cap();

	let plugins = boot::init_plugin_state(&loaded).await?;

	let providers = boot::build_metadata_providers(plugins.registry_boot_snap.as_ref());
	let symbolic = compile(loaded.files, &providers, &providers)?;
	tracing::info!(
		nodes = symbolic.nodes.len(),
		entries = symbolic.entries.len(),
		middlewares = symbolic.middlewares.len(),
		fetches = symbolic.fetches.len(),
		"compiled symbolic flow graph",
	);

	let crl_cache = init_crl_cache(&symbolic)?;
	let (security_cfg, security) = boot::init_security(&loaded.env, crl_cache.clone())?;

	let mw_factories = Arc::new(build_middleware_factories());

	// Open the ACME registry before linking so the AcmeChallenge fetch
	// factory can capture it.
	#[cfg(feature = "acme")]
	let acme_registry = acme_boot::open_registry_if_needed(&symbolic).await?;

	let fetch_factories = Arc::new(build_fetch_factories(
		security_cfg.crl_cache.clone(),
		#[cfg(feature = "acme")]
		acme_registry.clone(),
	));
	let initial_graph = boot::link_initial_graph(
		symbolic,
		&mw_factories,
		&fetch_factories,
		&security_cfg,
		plugins.registry_boot_snap.as_deref(),
		#[cfg(feature = "acme")]
		acme_registry.as_ref(),
	)?;
	let graph_swap: Arc<ArcSwap<FlowGraph>> = Arc::new(ArcSwap::new(initial_graph));
	tracing::info!("linked flow graph");

	// Pack the reload-pipeline state into one Arc so the mgmt verb
	// handler and the file-watcher loop both call `reload_once` against
	// the same shared bag. Cloned (refcount-only) into `MgmtState` and
	// `WatcherCtx` below.
	let reload_ctx = Arc::new(crate::reload::ReloadCtx::new(
		args.config_dir.clone(),
		&graph_swap,
		&mw_factories,
		&fetch_factories,
		&security_cfg,
		plugins.plugin_registry.as_ref(),
		#[cfg(feature = "wasm")]
		loaded.env.wasm_dir.clone(),
		#[cfg(feature = "wasm")]
		plugins.loaded_wasm.as_ref().map(|lw| &lw.runtime),
		#[cfg(feature = "wasm")]
		plugins.plugin_policies.as_ref(),
		#[cfg(feature = "acme")]
		acme_registry.as_ref(),
	));

	let (sink, broadcast_sink) = boot::compose_log_sink().await?;
	// Wire the same flow-log sink into the L1 security floor so
	// `SecurityState::maybe_warn` emits `FlowLogKind::SecurityLimit`
	// events alongside its tracing warn. Previously the kind was
	// dead code — the tracing warn fired but nothing reached the
	// structured flow log.
	security.set_log_sink(Arc::clone(&sink));
	let verbosity = Arc::new(VerbosityState::new());

	let (sigterm, sigint) = boot::install_signal_handlers();

	let listeners = Arc::new(ListenerSet::from_security_and_bind_config(
		Arc::clone(&security),
		BindConfig::from(&loaded.env),
	));

	// Phase 1 of file-watcher startup: build the FSEvents subscription
	// BEFORE calling `listeners.start`. Once a listener is reachable on
	// its bound port the operator can drop a new rule file and rightly
	// expect it to take effect; if the watcher subscribed late, that
	// drop's fs event could fire in the gap and be lost (FSEvents on
	// macOS does not replay events that pre-date subscription). Init
	// failure (typically permission-denied at the directory) is logged
	// and the daemon proceeds without auto-reload.
	let watcher_sub = match arm_watcher_subscription(args.config_dir.clone()) {
		Ok(s) => Some(s),
		Err(e) => {
			tracing::warn!(error = %e, "file watcher disabled — auto-reload unavailable");
			None
		}
	};

	listeners.start(&graph_swap, &verbosity, &sink);
	tracing::info!(active = listeners.len(), "listeners started");

	// `shutdown_trigger` is shared by the boot health watchdog (fires
	// it on total bind failure), the mgmt `shutdown` verb, and the
	// `wait_for_shutdown_signal` select loop.
	let shutdown_trigger = CancellationToken::new();

	#[cfg(feature = "acme")]
	if let Some(registry) = acme_registry.as_ref() {
		boot::spawn_acme_boot_tasks(registry, &graph_swap, &shutdown_trigger).await;
	}

	boot::spawn_boot_background_services(
		&security_cfg,
		&security,
		&listeners,
		&shutdown_trigger,
		loaded.env.boot_health_timeout_secs,
	);

	let _native_roots_refresh = spawn_native_roots_refresh(
		shutdown_trigger.clone(),
		loaded.env.native_roots_refresh_interval_secs,
	);

	// Phase 2 of file-watcher startup. Subscription was armed before
	// `listeners.start` so any event landing in the bind window is
	// already queued; the handler task picks them up on first poll.
	let watcher_cancel = CancellationToken::new();
	let watcher_handle = watcher_sub.map(|sub| {
		boot::spawn_file_watcher(
			sub,
			Arc::clone(&reload_ctx),
			Arc::clone(&listeners),
			Arc::clone(&verbosity),
			Arc::clone(&sink),
			watcher_cancel.clone(),
		)
	});

	let mgmt = boot::spawn_mgmt_plane(
		&reload_ctx,
		&listeners,
		&verbosity,
		&sink,
		&broadcast_sink,
		tracing_broadcast,
		&shutdown_trigger,
		&plugins,
		&loaded.env,
	)
	.await?;

	wait_for_shutdown_signal(boot::ShutdownContext {
		listeners,
		watcher_cancel,
		watcher_handle,
		mgmt,
		shutdown_trigger,
		sigterm,
		sigint,
		soft_drain: Duration::from_secs(loaded.env.drain_timeout_secs.into()),
	})
	.await;
	Ok(())
}

fn init_tracing(tail_layer: BroadcastTracingLayer, fallback_filter: &str) {
	// The fmt-to-stderr layer's filter source priority:
	//   1. `RUST_LOG` (operator ad-hoc override at the shell)
	//   2. `VANE_LOG_LEVEL` from `<config>/.env` or OS env (typed via
	//      `loaded.env.log_level`, supplied here as `fallback_filter`)
	//   3. The `"info"` default baked into `Env`.
	//
	// The broadcast layer is intentionally unfiltered so that `vane
	// tail log` shows every event the daemon emits regardless of how
	// noisy the operator's terminal is configured to be. Operators who
	// want to thin the stream client-side can pipe to `jq`.
	use tracing_subscriber::Layer;
	use tracing_subscriber::layer::SubscriberExt;
	use tracing_subscriber::util::SubscriberInitExt;
	let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
		EnvFilter::try_new(fallback_filter).unwrap_or_else(|_| EnvFilter::new("info"))
	});
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
/// `spec/crates/engine-tls.md` § _CRL_.
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

fn build_fetch_factories(
	crl_cache: Option<Arc<vane_engine::tls::CrlCache>>,
	#[cfg(feature = "acme")] acme_registry: Option<Arc<vane_engine::acme::ManagedCertRegistry>>,
) -> FetchFactories {
	let mut fetch = FetchFactories::new();
	vane_engine::fetch::l4_forward::register(&mut fetch);
	vane_engine::fetch::http_proxy::register(&mut fetch, crl_cache.clone());
	vane_engine::fetch::http_synthesize::register(&mut fetch);
	vane_engine::fetch::websocket_upgrade::register(&mut fetch, crl_cache);
	#[cfg(feature = "acme")]
	if let Some(registry) = acme_registry {
		vane_engine::fetch::acme_challenge::register(&mut fetch, registry);
	}
	fetch
}

/// Bind the Unix mgmt socket. Returns the spawned task's `JoinHandle`
/// or `None` if bind failed — the daemon continues serving traffic in
/// that case.
pub(crate) async fn bind_mgmt_unix_server(
	mgmt_state: Arc<MgmtState>,
	cancel: CancellationToken,
	socket: &std::path::Path,
) -> Option<tokio::task::JoinHandle<()>> {
	match vane_mgmt::spawn_unix_server(socket, mgmt_state, cancel).await {
		Ok(h) => {
			tracing::info!(socket = %socket.display(), "mgmt unix server bound");
			Some(h)
		}
		Err(e) => {
			tracing::warn!(
				socket = %socket.display(),
				error = %e,
				"mgmt unix server bind failed; daemon continues without mgmt",
			);
			None
		}
	}
}

/// Bind the HTTP-over-TCP mgmt transport per
/// `spec/crates/mgmt.md` § _Auth model_ and
/// `spec/crates/core.md` env-var section. Boot-validates the
/// `(VANE_MGMT_HTTP_PUBLIC, VANE_MGMT_HTTP_TOKEN)` pairing; bind
/// failures are fatal (the operator opted into HTTP transport, so a
/// missing port surfaces as a boot error rather than a silent
/// degradation).
///
/// Returns the per-bind task handles. Empty when the operator
/// disabled the transport via `VANE_MGMT_HTTP_PORT=`.
pub(crate) async fn bind_mgmt_http_server(
	mgmt_state: Arc<MgmtState>,
	cancel: CancellationToken,
	env: &vane_core::Env,
) -> Result<Vec<tokio::task::JoinHandle<()>>, Box<dyn std::error::Error + Send + Sync>> {
	let Some(port) = env.mgmt_http_port else {
		tracing::info!("mgmt http transport disabled (VANE_MGMT_HTTP_PORT is empty)");
		return Ok(Vec::new());
	};
	let public = env.mgmt_http_public;
	let token = env.mgmt_http_token.clone();

	// Boot validation table — see spec/crates/mgmt.md
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
	if !env.bind_ipv4 && !env.bind_ipv6 {
		return Err(
			"VANE_BIND_IPV4 and VANE_BIND_IPV6 are both disabled — no IP family available \
			 for management HTTP transport"
				.into(),
		);
	}

	let mut binds: Vec<SocketAddr> = Vec::new();
	if public {
		if env.bind_ipv4 {
			binds.push(format!("0.0.0.0:{port}").parse().expect("v4 wildcard"));
		}
		if env.bind_ipv6 {
			binds.push(format!("[::]:{port}").parse().expect("v6 wildcard"));
		}
	} else {
		if env.bind_ipv4 {
			binds.push(format!("127.0.0.1:{port}").parse().expect("v4 loopback"));
		}
		if env.bind_ipv6 {
			binds.push(format!("[::1]:{port}").parse().expect("v6 loopback"));
		}
	}

	let cfg = vane_mgmt::HttpServerConfig { binds, bearer_token: token.map(Into::into) };
	let handles = vane_mgmt::spawn_http_server(cfg, mgmt_state, cancel).await?;
	tracing::info!(count = handles.len(), port, public, "mgmt http server bound",);
	Ok(handles)
}

/// Boot-time health check. Spawns a background task that polls
/// `bound_count` against `expected` once per second until either every
/// listener has bound or the configured budget expires.
///
/// Outcomes after timeout:
/// - **Zero bound** (every listener gave up): set the
///   `BOOT_HEALTH_EXIT` flag and fire `shutdown_trigger`. The
///   shutdown drains through the
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
/// Background task that periodically re-reads the OS native trust
/// store and atomically swaps the cached snapshot. Runs every
/// `VANE_NATIVE_ROOTS_REFRESH_INTERVAL_SECS` (default 6h). A zero
/// interval disables the loop; operators relying on the
/// `reload_native_roots` mgmt verb for explicit refreshes can pin to
/// `0` so the daemon doesn't touch the keychain on its own.
pub(crate) fn spawn_native_roots_refresh(
	cancel: CancellationToken,
	interval_secs: u32,
) -> Option<tokio::task::JoinHandle<()>> {
	if interval_secs == 0 {
		tracing::info!("native trust store refresh disabled (interval=0)");
		return None;
	}
	let interval = Duration::from_secs(u64::from(interval_secs));
	Some(tokio::spawn(async move {
		let mut ticker = tokio::time::interval(interval);
		ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
		// First tick fires immediately; consume it so we don't refresh
		// during boot when the OnceLock has already cached the warm
		// load — the next tick is the first useful refresh.
		ticker.tick().await;
		loop {
			tokio::select! {
				biased;
				() = cancel.cancelled() => return,
				_ = ticker.tick() => {
					match vane_engine::tls::refresh_native_roots() {
						Ok(store) => tracing::info!(anchors = store.len(), "native trust store refreshed"),
						Err(e) => tracing::warn!(error = %e, "native trust store refresh failed"),
					}
				}
			}
		}
	}))
}

pub(crate) fn spawn_boot_health_watchdog(
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

async fn wait_for_shutdown_signal(ctx: boot::ShutdownContext) {
	let boot::ShutdownContext {
		listeners,
		watcher_cancel,
		watcher_handle,
		mgmt,
		shutdown_trigger,
		mut sigterm,
		mut sigint,
		soft_drain,
	} = ctx;
	let drain = tokio::select! {
		_ = sigterm.recv() => {
			tracing::info!(drain_secs = soft_drain.as_secs(), "SIGTERM received — soft drain");
			soft_drain
		}
		_ = sigint.recv() => {
			tracing::info!("SIGINT received — immediate shutdown");
			Duration::from_secs(0)
		}
		() = shutdown_trigger.cancelled() => {
			tracing::info!(drain_secs = soft_drain.as_secs(), "mgmt shutdown verb received — soft drain");
			soft_drain
		}
	};
	watcher_cancel.cancel();
	mgmt.cancel.cancel();
	if let Some(h) = watcher_handle {
		let _ = h.await;
	}
	if let Some(h) = mgmt.unix_handle {
		let _ = h.await;
	}
	for h in mgmt.http_handles {
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
pub(crate) fn collect_missing_plugin_refs(
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
