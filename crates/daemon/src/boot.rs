//! Daemon boot phases extracted from `main::run`. Each free function
//! corresponds to one named phase of the startup sequence; `run`'s body
//! reads top-to-bottom as a roadmap of phase calls instead of inline
//! procedure.
//!
//! See `spec/crates/daemon.md` for the full boot ordering rationale.

use std::sync::Arc;

use arc_swap::ArcSwap;
use tokio::signal::unix::{Signal, SignalKind, signal};
use tokio_util::sync::CancellationToken;
#[cfg(feature = "wasm")]
use vane_core::PluginPolicyTable;
use vane_core::config::{Env, LoadedConfig};
use vane_core::{Error, FlowLogSink, SymbolicFlowGraph};
use vane_engine::factories::{FetchFactories, MiddlewareFactories};
use vane_engine::flow_graph::{FlowGraph, LinkError, PluginRegistry};
use vane_engine::flow_log_sink::{BroadcastSink, FanoutSink, default_sink_from_env};
use vane_engine::{ListenerSet, SecurityConfig, SecurityState, VerbosityState};

use crate::providers::MetadataProviders;
#[cfg(feature = "wasm")]
use crate::wasm_loader;
use crate::{collect_missing_plugin_refs, spawn_boot_health_watchdog};

/// Plugin-related boot state derived from a one-shot WASM scan plus
/// the rule-set ref-check. Threaded forward through `MetadataProviders`
/// construction, the initial link, and `ReloadCtx`.
pub(crate) struct PluginBootState {
	pub plugin_registry: Option<Arc<ArcSwap<PluginRegistry>>>,
	pub registry_boot_snap: Option<Arc<PluginRegistry>>,
	#[cfg(feature = "wasm")]
	pub loaded_wasm: Option<wasm_loader::LoadedWasm>,
	#[cfg(feature = "wasm")]
	pub plugin_policies: Option<Arc<ArcSwap<PluginPolicyTable>>>,
}

/// Phase: install rustls's process-wide crypto provider, the daemon-wide
/// TLS session ticketer, and the metrics recorder. All three are
/// idempotent (a second call is a logged no-op) so the daemon main and
/// any test harness can both invoke this without coordination.
///
/// Order matters: ticketer needs the crypto provider's RNG; the link
/// step that follows reads the ticketer into each listener's
/// `ServerConfig`. Failure on the ticketer / metrics installs is fatal
/// (kernel CSPRNG unavailable / metrics double-init) and panics the
/// daemon at boot — both are unrecoverable.
pub(crate) fn install_global_runtime() {
	vane_engine::crypto::install_default_provider();
	vane_engine::tls::install_default_ticketer().expect("install rustls session ticketer");
	vane_engine::metrics::install_recorder().expect("install metrics recorder");
}

/// Phase: surface the operator-tunable CGI concurrency cap. Read here
/// (rather than at first CGI request) so the resolved value shows up in
/// the startup log even when no CGI traffic has arrived yet.
/// Spec: `spec/crates/engine.md` § _Concurrency cap_.
pub(crate) fn log_cgi_concurrency_cap() {
	let cgi_max_concurrent = std::env::var("VANE_CGI_MAX_CONCURRENT")
		.ok()
		.and_then(|s| s.parse::<usize>().ok())
		.filter(|n| *n > 0)
		.unwrap_or(100);
	tracing::info!(cgi_max_concurrent, "cgi concurrency cap resolved");
}

/// Phase: WASM boot scan + plugin-ref check. Returns the registry +
/// policy handles that thread through `MetadataProviders`, the initial
/// link, and `ReloadCtx`. Refuses to start if any rule references a
/// plugin that the boot scan didn't load — the curated list lets
/// operators fix every reference in one cycle rather than discover them
/// one-by-one through compile errors.
///
/// # Errors
/// Returns a stringly error when missing plugin refs are detected.
pub(crate) async fn init_plugin_state(
	loaded: &LoadedConfig,
) -> Result<PluginBootState, Box<dyn std::error::Error + Send + Sync>> {
	#[cfg(feature = "wasm")]
	let loaded_wasm = wasm_loader::load_all(&loaded.env.wasm_dir).await;

	#[cfg(feature = "wasm")]
	let plugin_registry: Option<Arc<ArcSwap<PluginRegistry>>> =
		loaded_wasm.as_ref().map(|lw| Arc::clone(&lw.registry));
	#[cfg(not(feature = "wasm"))]
	let plugin_registry: Option<Arc<ArcSwap<PluginRegistry>>> = None;

	#[cfg(feature = "wasm")]
	let plugin_policies: Option<Arc<ArcSwap<PluginPolicyTable>>> =
		loaded_wasm.as_ref().map(|lw| Arc::clone(&lw.policies));

	let registry_boot_snap = plugin_registry.as_ref().map(|s| s.load_full());

	let missing_plugins = collect_missing_plugin_refs(&loaded.files, registry_boot_snap.as_ref());
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

	Ok(PluginBootState {
		plugin_registry,
		registry_boot_snap,
		#[cfg(feature = "wasm")]
		loaded_wasm,
		#[cfg(feature = "wasm")]
		plugin_policies,
	})
}

/// Phase: build [`MetadataProviders`] from the boot-time plugin
/// registry snapshot. Used by `compile` to resolve `<module>:<export>`
/// references against the loaded plugin set.
pub(crate) fn build_metadata_providers(
	registry_boot_snap: Option<&Arc<PluginRegistry>>,
) -> MetadataProviders {
	match registry_boot_snap {
		#[cfg(feature = "wasm")]
		Some(reg) => MetadataProviders::with_plugins(Arc::clone(reg)),
		#[cfg(not(feature = "wasm"))]
		Some(_) => MetadataProviders::new(),
		None => MetadataProviders::new(),
	}
}

/// Phase: validate L1 security floors against operator env, build the
/// daemon-scoped [`SecurityConfig`] + [`SecurityState`], and surface
/// the resolved values in the startup log. The CRL cache (built by the
/// caller from the symbolic graph) folds into the config so per-link
/// handshakes can read it.
///
/// # Errors
/// Surfaces validation failures from [`SecurityConfig::new`].
pub(crate) fn init_security(
	env: &Env,
	crl_cache: Option<Arc<vane_engine::tls::CrlCache>>,
) -> Result<(Arc<SecurityConfig>, Arc<SecurityState>), Error> {
	let mut security_cfg_inner = SecurityConfig::new(env)?;
	security_cfg_inner.crl_cache = crl_cache;
	let security_cfg = Arc::new(security_cfg_inner);
	let security = Arc::new(SecurityState::new((*security_cfg).clone()));
	tracing::info!(
		header_timeout_secs = security_cfg.header_timeout.as_secs(),
		max_conn_per_ip = security_cfg.max_conn_per_ip,
		max_total_conns = security_cfg.max_total_conns,
		crl_cache = security_cfg.crl_cache.is_some(),
		"L1 security floor configured",
	);
	Ok((security_cfg, security))
}

/// Phase: link the symbolic flow graph against the engine's middleware
/// and fetch factories, returning the initial concrete [`FlowGraph`].
/// The `cfg(feature = "acme")` branch routes through
/// [`FlowGraph::link_with_acme`] so the boot graph picks up
/// `tls.managed` populators; the non-acme branch picks
/// `link_with_plugins` or `link_with_security` based on whether any
/// plugins are loaded.
///
/// # Errors
/// Surfaces whatever the underlying link returns (factory rejection,
/// kind mismatch, feature-disabled, etc.).
pub(crate) fn link_initial_graph(
	symbolic: Arc<SymbolicFlowGraph>,
	mw_factories: &MiddlewareFactories,
	fetch_factories: &FetchFactories,
	security_cfg: &Arc<SecurityConfig>,
	registry_boot_snap: Option<&PluginRegistry>,
	#[cfg(feature = "acme")] acme_registry: Option<&Arc<vane_engine::acme::ManagedCertRegistry>>,
) -> Result<Arc<FlowGraph>, LinkError> {
	#[cfg(feature = "acme")]
	{
		FlowGraph::link_with_acme(
			symbolic,
			mw_factories,
			registry_boot_snap,
			fetch_factories,
			Arc::clone(security_cfg),
			acme_registry,
		)
	}
	#[cfg(not(feature = "acme"))]
	{
		match registry_boot_snap {
			Some(reg) => FlowGraph::link_with_plugins(
				symbolic,
				mw_factories,
				reg,
				fetch_factories,
				Arc::clone(security_cfg),
			),
			None => FlowGraph::link_with_security(
				symbolic,
				mw_factories,
				fetch_factories,
				Arc::clone(security_cfg),
			),
		}
	}
}

/// Phase: compose the runtime flow-log sink. Default sink (env-driven
/// ring buffer ± optional file sink) wraps in a `FanoutSink` alongside
/// a `BroadcastSink` so the mgmt `tail_flow` verb has a live event
/// source. Returns both sinks; `MgmtState` keeps the broadcast handle
/// directly so handlers can call `subscribe()` without going through
/// the fanout.
///
/// # Errors
/// Surfaces I/O failure when the default sink (file path) fails to open.
pub(crate) async fn compose_log_sink()
-> Result<(Arc<dyn FlowLogSink>, Arc<BroadcastSink>), Box<dyn std::error::Error + Send + Sync>> {
	let default_sink = default_sink_from_env().await?;
	let broadcast_sink = Arc::new(BroadcastSink::new());
	let sink: Arc<dyn FlowLogSink> = Arc::new(FanoutSink::new(vec![
		default_sink,
		Arc::clone(&broadcast_sink) as Arc<dyn FlowLogSink>,
	]));
	Ok((sink, broadcast_sink))
}

/// Phase: install POSIX shutdown-signal streams BEFORE any listener
/// starts. From this point on SIGTERM / SIGINT are queued onto the
/// returned streams instead of taking their default termination
/// disposition. Awaited at the end of `main::run` via
/// `wait_for_shutdown_signal`.
///
/// # Panics
/// Panics if the kernel-level signal handler install fails — that's
/// unrecoverable.
pub(crate) fn install_signal_handlers() -> (Signal, Signal) {
	let sigterm = signal(SignalKind::terminate()).expect("install SIGTERM handler");
	let sigint = signal(SignalKind::interrupt()).expect("install SIGINT handler");
	(sigterm, sigint)
}

/// Phase: kick off ACME first-time issuance for every `tls.managed`
/// SNI without a cached cert, auto-bind a synthetic `:80` listener if
/// the operator's config has none, and start the renewal scheduler.
/// All three are fire-and-forget; ACME failures surface via
/// `tracing::error!` and don't abort boot. The renewal scheduler ticks
/// every 5 minutes per `spec/crates/engine-acme.md` § _Renewal triggers_
/// and dies with the runtime on shutdown.
#[cfg(feature = "acme")]
pub(crate) async fn spawn_acme_boot_tasks(
	registry: &Arc<vane_engine::acme::ManagedCertRegistry>,
	graph_swap: &Arc<ArcSwap<FlowGraph>>,
	shutdown_trigger: &CancellationToken,
) {
	let graph = graph_swap.load_full();
	let _issuance_handles =
		crate::acme_boot::kick_off_managed_issuance(registry, &graph, shutdown_trigger);
	let _auto_bind_handles =
		crate::acme_boot::maybe_auto_bind_port_80(Arc::clone(registry), &graph, shutdown_trigger).await;
	let _scheduler_handle = registry.spawn_scheduler();
}

/// Phase 2 of file-watcher startup: build [`WatcherCtx`] from the
/// active reload + listener handles and spawn the handler task that
/// drains queued reload signals. Subscription is consumed; the
/// returned `JoinHandle` lives until `cancel` fires.
pub(crate) fn spawn_file_watcher(
	sub: notify_twophase::Subscription,
	reload: Arc<crate::reload::ReloadCtx>,
	listeners: Arc<ListenerSet>,
	verbosity: Arc<VerbosityState>,
	log_sink: Arc<dyn FlowLogSink>,
	cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
	let watcher_ctx = Arc::new(crate::watcher::WatcherCtx { reload, listeners, verbosity, log_sink });
	let h = crate::watcher::spawn_watcher_handler(sub, watcher_ctx, cancel);
	tracing::info!("file watcher armed");
	h
}

/// Handles produced by [`spawn_mgmt_plane`]. Threaded into
/// `wait_for_shutdown_signal` so a triggered shutdown can cancel the
/// mgmt cancel token, await the unix socket task, and abort each HTTP
/// listener task.
pub(crate) struct MgmtPlaneHandles {
	pub cancel: CancellationToken,
	pub unix_handle: Option<tokio::task::JoinHandle<()>>,
	pub http_handles: Vec<tokio::task::JoinHandle<()>>,
}

/// Phase: build [`MgmtState`] from the live daemon handles, bind the
/// Unix mgmt socket, bind the HTTP mgmt listeners. Bind failures on
/// either transport are logged and the daemon continues serving traffic
/// without that flavour of mgmt — operators can fix the path / config
/// and restart.
///
/// # Errors
/// Surfaces the HTTP bind path's `Result` (typed once it gains real
/// failure modes); the Unix bind path is internally infallible.
#[allow(
	clippy::too_many_arguments,
	reason = "boot orchestrator wiring eight independent daemon-wide handles into MgmtState construction + two server binds"
)]
pub(crate) async fn spawn_mgmt_plane(
	reload: &Arc<crate::reload::ReloadCtx>,
	listeners: &Arc<ListenerSet>,
	verbosity: &Arc<VerbosityState>,
	log_sink: &Arc<dyn FlowLogSink>,
	broadcast: &Arc<vane_engine::flow_log_sink::BroadcastSink>,
	tracing_broadcast: tracing_broadcast::BroadcastTracingLayer,
	shutdown_trigger: &CancellationToken,
	plugins: &PluginBootState,
	env: &Env,
) -> Result<MgmtPlaneHandles, Box<dyn std::error::Error + Send + Sync>> {
	#[cfg(feature = "wasm")]
	let wasm_pool_stats = plugins
		.loaded_wasm
		.as_ref()
		.map(|lw| Arc::clone(&lw.runtime) as Arc<dyn vane_core::WasmPoolStats>);
	#[cfg(not(feature = "wasm"))]
	let wasm_pool_stats: Option<Arc<dyn vane_core::WasmPoolStats>> = {
		let _ = plugins;
		None
	};

	let mgmt_state = Arc::new(crate::mgmt_handlers::MgmtState {
		started_at: std::time::Instant::now(),
		reload: Arc::clone(reload),
		listeners: Arc::clone(listeners),
		verbosity: Arc::clone(verbosity),
		log_sink: Arc::clone(log_sink),
		broadcast: Arc::clone(broadcast),
		tracing_broadcast,
		shutdown_trigger: shutdown_trigger.clone(),
		wasm_pool_stats,
	});
	let cancel = CancellationToken::new();
	let unix_handle =
		crate::bind_mgmt_unix_server(Arc::clone(&mgmt_state), cancel.clone(), &env.mgmt_unix).await;
	let http_handles =
		crate::bind_mgmt_http_server(Arc::clone(&mgmt_state), cancel.clone(), env).await?;
	Ok(MgmtPlaneHandles { cancel, unix_handle, http_handles })
}

/// Phase: spawn the post-`listeners.start` background services that
/// run for the daemon's lifetime — CRL URL refresher, L1 security
/// state cleanup, and the boot-health watchdog. Each task observes
/// `shutdown_trigger` (or dies with the runtime when the daemon exits).
pub(crate) fn spawn_boot_background_services(
	security_cfg: &SecurityConfig,
	security: &Arc<SecurityState>,
	listeners: &Arc<ListenerSet>,
	shutdown_trigger: &CancellationToken,
	boot_health_timeout_secs: u32,
) {
	if let Some(cache) = &security_cfg.crl_cache {
		cache.spawn_refresher(shutdown_trigger);
	}
	Arc::clone(security).spawn_cleanup(shutdown_trigger.clone());

	let expected_listener_count = listeners.expected_count();
	if expected_listener_count == 0 {
		tracing::warn!("graph has no listener entries; daemon will serve nothing");
	} else {
		spawn_boot_health_watchdog(
			Arc::clone(listeners),
			shutdown_trigger.clone(),
			expected_listener_count,
			boot_health_timeout_secs,
		);
	}
}
