//! Concrete dispatcher for the mgmt verbs. Holds `Arc` references to
//! daemon-state primitives — graph swap, listener set, factories, log
//! sink, verbosity, shutdown trigger — so each handler can answer
//! queries and drive actions against the live daemon without the
//! `vane-mgmt` crate needing to depend on `vane-engine`.
//!
//! Reload path mirrors `watcher.rs`: on a successful swap we call
//! `ListenerSet::reconcile` so any added/removed `entries` addresses
//! get bound or background-drained. The two reload sources (watcher,
//! mgmt verb) thus produce equivalent runtime state.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use vane_core::compile::compile;
use vane_core::{FlowLogEvent, FlowLogSink, WasmPoolStats};
use vane_engine::ListenerSet;
use vane_engine::SecurityConfig;
use vane_engine::VerbosityState;
use vane_engine::factories::{FetchFactories, MiddlewareFactories};
use vane_engine::flow_graph::FlowGraph;
use vane_engine::flow_log_sink::BroadcastSink;
use vane_engine::tracing_broadcast::{BroadcastTracingLayer, TracingFrame};
use vane_mgmt::protocol::{Request, WireError, WireErrorKind};
use vane_mgmt::server::{DispatchOutcome, EventStream, Handler};
use vane_mgmt::verb::{
	CompileDryRunArgs, CompileDryRunResult, ConnectionInfo, GetConfigResult, GetConnectionsResult,
	GetMetricsArgs, GetMetricsResult, GetPoolsResult, GetUpstreamsResult, ListenerStatus, PingResult,
	ReloadResult, ShutdownResult, StatsResult, TcpUpstreamEntry, VERB_COMPILE_DRY_RUN,
	VERB_FORCE_RENEW, VERB_GET_CERTS, VERB_GET_CONFIG, VERB_GET_CONNECTIONS, VERB_GET_METRICS,
	VERB_GET_POOLS, VERB_GET_UPSTREAMS, VERB_PING, VERB_RELOAD, VERB_SHUTDOWN, VERB_STATS,
	VERB_TAIL_FLOW, VERB_TAIL_LOG, WasmPoolEntry,
};

use crate::providers::MetadataProviders;
use crate::reload::{ReloadOutcome, reload_once};

/// Live daemon state visible to mgmt verb handlers. Built once during
/// boot in `main::run` and shared by every accepted mgmt connection
/// through `Arc<MgmtState>`.
pub(crate) struct MgmtState {
	pub started_at: Instant,
	pub graph_swap: Arc<ArcSwap<FlowGraph>>,
	pub listeners: Arc<ListenerSet>,
	pub mw_factories: Arc<MiddlewareFactories>,
	pub fetch_factories: Arc<FetchFactories>,
	pub config_dir: PathBuf,
	pub verbosity: Arc<VerbosityState>,
	pub log_sink: Arc<dyn FlowLogSink>,
	/// Live broadcast handle. `tail_flow` subscribes here for
	/// incident-time event streaming.
	pub broadcast: Arc<BroadcastSink>,
	/// Tracing layer that broadcasts every emitted event. `tail_log`
	/// subscribes here. Cheap to clone (wraps a [`broadcast::Sender`]).
	pub tracing_broadcast: BroadcastTracingLayer,
	pub security_cfg: Arc<SecurityConfig>,
	/// Fired by the `shutdown` verb. The daemon's main signal loop
	/// awaits this alongside SIGINT/SIGTERM.
	pub shutdown_trigger: CancellationToken,
	/// Plumbing for the `get_pools` verb's WASM section. `None` when
	/// the daemon is built without the `wasm` feature, or when a
	/// `wasm`-built daemon has not yet instantiated the runtime —
	/// `get_pools` then returns an empty `wasm` list, and CGI / TCP
	/// pool data still flows through.
	pub wasm_pool_stats: Option<Arc<dyn WasmPoolStats>>,
	/// Plugin registry built at boot from `<wasm_dir>/*.wasm`. `None`
	/// when the daemon was built without the `wasm` feature, or when
	/// the boot scan loaded nothing. Reload + `compile_dry_run`
	/// thread this through so plugin references resolve consistently
	/// across reload cycles without re-scanning the filesystem
	/// (live-add of new modules is a daemon-restart operation).
	///
	/// `vane_engine::flow_graph::PluginRegistry` itself is always
	/// available (not feature-gated in vane-engine), so the field
	/// stays unconditional even when daemon's `wasm` is off.
	pub plugin_registry: Option<Arc<arc_swap::ArcSwap<vane_engine::flow_graph::PluginRegistry>>>,
	/// Operator-owned plugin policy table held in `ArcSwap` so reload
	/// publishes a fresh table atomically. Only present when the
	/// daemon was built with `wasm` and the boot scan succeeded.
	#[cfg(feature = "wasm")]
	pub plugin_policies: Option<Arc<arc_swap::ArcSwap<vane_core::PluginPolicyTable>>>,
	/// Runtime handle the reload pipeline reuses when re-scanning
	/// `<wasm_dir>` on every reload. `None` when wasm is off.
	#[cfg(feature = "wasm")]
	pub wasm_runtime: Option<Arc<vane_wasm::WasmtimeRuntime>>,
	/// `<config_dir>/wasm` — re-scanned on every reload. `Option` is
	/// not used here because the path is always derivable from the
	/// daemon's `Env`; absent / empty dir is handled inside
	/// `wasm_loader::reload_dir`.
	#[cfg(feature = "wasm")]
	pub wasm_dir: std::path::PathBuf,
	/// Daemon-scoped ACME registry (per `spec/crates/engine-acme.md` § _Architecture_).
	/// Threaded into `reload_once` so post-reload `FlowGraph::link`
	/// re-attaches the same registry to fresh per-listener
	/// `ManagedCertPopulator`s — accounts and issued certs survive
	/// reloads. `None` when the daemon was built without `acme` or
	/// when boot found no `tls.managed` rules.
	#[cfg(feature = "acme")]
	pub acme_registry: Option<Arc<vane_engine::acme::ManagedCertRegistry>>,
}

#[async_trait]
impl Handler for MgmtState {
	async fn dispatch(&self, req: Request) -> DispatchOutcome {
		// Streaming verbs are dispatched first because their return type
		// is `Stream`, not `OneShot`. Everything else funnels through the
		// shared one-shot path below.
		if req.verb == VERB_TAIL_FLOW {
			let rx = self.broadcast.subscribe();
			return DispatchOutcome::Stream(Box::new(FlowLogStream { rx }));
		}
		if req.verb == VERB_TAIL_LOG {
			let rx = self.tracing_broadcast.subscribe();
			return DispatchOutcome::Stream(Box::new(TailLogStream { rx }));
		}
		let result: Result<serde_json::Value, WireError> = match req.verb.as_str() {
			VERB_PING => self.handle_ping(),
			VERB_STATS => self.handle_stats(),
			VERB_SHUTDOWN => self.handle_shutdown(),
			VERB_GET_CONFIG => self.handle_get_config(),
			VERB_RELOAD => self.handle_reload().await,
			VERB_COMPILE_DRY_RUN => self.handle_compile_dry_run(req.args),
			VERB_GET_CONNECTIONS => self.handle_get_connections(),
			VERB_GET_METRICS => self.handle_get_metrics(req.args),
			VERB_GET_POOLS => self.handle_get_pools(),
			VERB_GET_UPSTREAMS => self.handle_get_upstreams(),
			vane_mgmt::verb::VERB_POOL_DRAIN => Self::handle_pool_drain(req.args),
			#[cfg(feature = "acme")]
			VERB_FORCE_RENEW => self.handle_force_renew(req.args),
			#[cfg(feature = "acme")]
			VERB_GET_CERTS => self.handle_get_certs(),
			#[cfg(not(feature = "acme"))]
			VERB_FORCE_RENEW | VERB_GET_CERTS => Err(WireError {
				kind: WireErrorKind::UnknownVerb,
				message: format!(
					"verb {:?} requires the daemon to be built with the `acme` feature",
					req.verb,
				),
			}),
			other => Err(WireError {
				kind: WireErrorKind::UnknownVerb,
				message: format!("unknown verb {other:?}"),
			}),
		};
		DispatchOutcome::OneShot(result)
	}
}

/// Streaming source for the `tail_flow` verb. Wraps a per-call
/// broadcast receiver; encodes each `FlowLogEvent` to JSON; surfaces
/// `Lagged` as a synthetic sentinel event so operators can see when
/// they're getting a sampled view.
pub(crate) struct FlowLogStream {
	rx: broadcast::Receiver<FlowLogEvent>,
}

/// Streaming source for the `tail_log` verb. Same pattern as
/// `FlowLogStream` but the upstream channel carries [`TracingFrame`]s
/// (RUST_LOG-gated tracing events).
pub(crate) struct TailLogStream {
	rx: broadcast::Receiver<TracingFrame>,
}

#[async_trait]
impl EventStream for TailLogStream {
	async fn next_event(&mut self) -> Option<serde_json::Value> {
		loop {
			match self.rx.recv().await {
				Ok(frame) => match serde_json::to_value(&frame) {
					Ok(v) => return Some(v),
					Err(e) => {
						tracing::warn!(?e, "tail_log frame encode failed; dropping");
					}
				},
				Err(broadcast::error::RecvError::Lagged(n)) => {
					tracing::warn!(dropped = n, "tail_log subscriber lagged");
					return Some(serde_json::json!({
						"kind": "lagged",
						"dropped": n,
					}));
				}
				Err(broadcast::error::RecvError::Closed) => return None,
			}
		}
	}
}

#[async_trait]
impl EventStream for FlowLogStream {
	async fn next_event(&mut self) -> Option<serde_json::Value> {
		loop {
			match self.rx.recv().await {
				Ok(event) => match serde_json::to_value(&event) {
					Ok(v) => return Some(v),
					Err(e) => {
						// A FlowLogEvent that fails to serialize is a bug
						// somewhere in the engine — log and skip rather
						// than tearing down the whole stream.
						tracing::warn!(?e, "flow log event encode failed; dropping frame");
					}
				},
				Err(broadcast::error::RecvError::Lagged(n)) => {
					// Slow subscriber dropped n events. Surface a
					// synthetic sentinel so the operator notices the
					// gap rather than seeing a "smooth" stream.
					tracing::warn!(dropped = n, "tail_flow subscriber lagged");
					return Some(serde_json::json!({
						"kind": "lagged",
						"dropped": n,
					}));
				}
				Err(broadcast::error::RecvError::Closed) => return None,
			}
		}
	}
}

fn parse_args<A: for<'de> serde::Deserialize<'de>>(
	value: serde_json::Value,
) -> Result<A, WireError> {
	serde_json::from_value(value)
		.map_err(|e| WireError { kind: WireErrorKind::BadArgs, message: format!("args: {e}") })
}

/// Read the CGI semaphore snapshot. `None` when the `cgi` feature is
/// off, or when the semaphore has not yet been lazily initialised
/// (no CGI request has fired). Read-only — never triggers
/// first-init, so `get_pools` on a cold daemon does not bake
/// `VANE_CGI_MAX_CONCURRENT` into a process-wide constant.
#[cfg(feature = "cgi")]
fn cgi_pool_entry() -> Option<vane_mgmt::verb::CgiPoolEntry> {
	vane_engine::fetch::cgi::pool_stats().map(|s| vane_mgmt::verb::CgiPoolEntry {
		cap: s.cap,
		available: s.available,
		in_use: s.in_use,
		total_allocations: s.total_allocations,
		failures: s.failures,
	})
}

#[cfg(not(feature = "cgi"))]
fn cgi_pool_entry() -> Option<vane_mgmt::verb::CgiPoolEntry> {
	None
}

/// Snapshot the daemon-level QUIC pool. Empty when the `h3` feature
/// is off; otherwise reports one entry per cached `(addr, tls)` pair.
#[cfg(feature = "h3")]
fn quic_upstream_entries() -> Vec<vane_mgmt::verb::QuicUpstreamEntry> {
	vane_engine::fetch::quic_pool::snapshot()
		.into_iter()
		.map(|s| vane_mgmt::verb::QuicUpstreamEntry {
			remote_addr: s.remote_addr,
			sni: s.sni,
			alpn: s.alpn,
			fingerprint_id: s.fingerprint_id,
		})
		.collect()
}

#[cfg(not(feature = "h3"))]
fn quic_upstream_entries() -> Vec<vane_mgmt::verb::QuicUpstreamEntry> {
	Vec::new()
}

#[cfg(feature = "h3")]
fn quic_drain_by_id(id: &str) -> usize {
	vane_engine::fetch::quic_pool::drain_by_fingerprint_id(id)
}

#[cfg(not(feature = "h3"))]
fn quic_drain_by_id(_id: &str) -> usize {
	0
}

fn json<R: serde::Serialize>(r: &R) -> Result<serde_json::Value, WireError> {
	serde_json::to_value(r)
		.map_err(|e| WireError { kind: WireErrorKind::Internal, message: format!("encode: {e}") })
}

/// Render a [`std::time::SystemTime`] as RFC 3339 / ISO 8601 UTC.
/// Used by `get_certs` to format the wire-shape timestamp fields
/// per `spec/crates/engine-acme.md` § _get_certs response shape_. Falls back to
/// `"<invalid>"` if the timestamp is pre-1970 (defensive — should
/// never happen for ACME-issued certs, but the API accepts arbitrary
/// `SystemTime` so we don't panic).
#[cfg(feature = "acme")]
fn rfc3339(t: std::time::SystemTime) -> String {
	let dur = match t.duration_since(std::time::UNIX_EPOCH) {
		Ok(d) => d,
		Err(_) => return "<invalid>".to_owned(),
	};
	let secs = i64::try_from(dur.as_secs()).unwrap_or(i64::MAX);
	time::OffsetDateTime::from_unix_timestamp(secs)
		.ok()
		.and_then(|dt| dt.format(&time::format_description::well_known::Rfc3339).ok())
		.unwrap_or_else(|| "<invalid>".to_owned())
}

/// Translate [`vane_engine::acme::CertStatus`] into the wire-shape
/// lowercase string per `spec/crates/engine-acme.md` § _get_certs response shape_.
#[cfg(feature = "acme")]
fn status_label(state: &vane_engine::acme::CertState) -> String {
	use vane_engine::acme::CertStatus;
	match state.status {
		CertStatus::Valid => "valid".to_owned(),
		CertStatus::Renewing => "renewing".to_owned(),
		CertStatus::Failed => "failed".to_owned(),
		CertStatus::Limited => "limited".to_owned(),
	}
}

/// Compute the wire-shape `ocsp_status` value from the cert's
/// stored OCSP fields. Three branches map directly onto operator
/// dashboards:
///
/// - `"stapled"`: a fresh response is cached and rustls ships it
///   on every handshake.
/// - `"no_staple"`: the cert advertises no AIA OCSP URL, so OCSP
///   isn't applicable.
/// - `"fetch_failed"`: the AIA URL is known but the most recent
///   fetch didn't produce usable bytes; the scheduler will retry.
#[cfg(feature = "acme")]
fn ocsp_status_label(staple: Option<&[u8]>, aia_url: Option<&str>) -> String {
	match (staple, aia_url) {
		(Some(_), _) => "stapled".to_owned(),
		(None, Some(_)) => "fetch_failed".to_owned(),
		(None, None) => "no_staple".to_owned(),
	}
}

fn hex32(bytes: &[u8; 32]) -> String {
	use std::fmt::Write as _;
	let mut s = String::with_capacity(64);
	for b in bytes {
		let _ = write!(s, "{b:02x}");
	}
	s
}

impl MgmtState {
	// `&self` is used by `dispatch` as the method receiver — the body
	// just doesn't read state. Suppressing the lint instead of moving
	// to an associated function keeps every handler shape consistent.
	#[allow(clippy::unused_self)]
	fn handle_ping(&self) -> Result<serde_json::Value, WireError> {
		json(&PingResult { pong: true, version: env!("CARGO_PKG_VERSION").to_string() })
	}

	fn handle_stats(&self) -> Result<serde_json::Value, WireError> {
		let listeners = self.listener_status();
		let graph = self.graph_swap.load();
		let hex = hex32(&graph.meta().version_hash);
		json(&StatsResult {
			uptime_ms: u64::try_from(self.started_at.elapsed().as_millis()).unwrap_or(u64::MAX),
			graph_version_hash: hex,
			listeners,
			flow_log_subscribers: self.broadcast.subscriber_count(),
			tracing_log_subscribers: self.tracing_broadcast.subscriber_count(),
		})
	}

	fn handle_shutdown(&self) -> Result<serde_json::Value, WireError> {
		self.shutdown_trigger.cancel();
		json(&ShutdownResult { draining: true })
	}

	fn handle_get_config(&self) -> Result<serde_json::Value, WireError> {
		let graph = self.graph_swap.load();
		let serialized = serde_json::to_value(graph.symbolic().as_ref()).map_err(|e| WireError {
			kind: WireErrorKind::Internal,
			message: format!("symbolic: {e}"),
		})?;
		json(&GetConfigResult { graph: serialized })
	}

	async fn handle_reload(&self) -> Result<serde_json::Value, WireError> {
		let outcome = reload_once(
			&self.config_dir,
			#[cfg(feature = "wasm")]
			Some(self.wasm_dir.as_path()),
			#[cfg(feature = "wasm")]
			self.wasm_runtime.as_ref(),
			&self.graph_swap,
			&self.mw_factories,
			&self.fetch_factories,
			&self.security_cfg,
			self.plugin_registry.as_ref(),
			#[cfg(feature = "wasm")]
			self.plugin_policies.as_ref(),
			#[cfg(feature = "acme")]
			self.acme_registry.as_ref(),
		)
		.await;
		match outcome {
			Ok(ReloadOutcome::Swapped { hash }) => {
				// Match the watcher's post-swap behaviour: reconcile the
				// listener set with the new graph's `entries`.
				self.listeners.reconcile(
					Arc::clone(&self.graph_swap),
					Arc::clone(&self.verbosity),
					Arc::clone(&self.log_sink),
				);
				json(&ReloadResult::Swapped { hash: hex32(&hash) })
			}
			Ok(ReloadOutcome::Unchanged { hash }) => {
				json(&ReloadResult::Unchanged { hash: hex32(&hash) })
			}
			Err(e) => Err(WireError { kind: WireErrorKind::Internal, message: format!("reload: {e}") }),
		}
	}

	fn handle_compile_dry_run(
		&self,
		args: serde_json::Value,
	) -> Result<serde_json::Value, WireError> {
		let args: CompileDryRunArgs = parse_args(args)?;
		let dir = PathBuf::from(args.config_dir);
		let loaded = vane_core::config::load(&dir)
			.map_err(|e| WireError { kind: WireErrorKind::BadArgs, message: format!("load: {e}") })?;
		let registry_snap = self.plugin_registry.as_ref().map(|s| s.load_full());
		let providers = match registry_snap.as_ref() {
			#[cfg(feature = "wasm")]
			Some(reg) => MetadataProviders::with_plugins(Arc::clone(reg)),
			#[cfg(not(feature = "wasm"))]
			Some(_) => MetadataProviders::new(),
			None => MetadataProviders::new(),
		};
		let symbolic = compile(loaded.files, &providers, &providers)
			.map_err(|e| WireError { kind: WireErrorKind::BadArgs, message: format!("compile: {e}") })?;
		let value = serde_json::to_value(&symbolic).map_err(|e| WireError {
			kind: WireErrorKind::Internal,
			message: format!("symbolic: {e}"),
		})?;
		json(&CompileDryRunResult { graph: value })
	}

	fn handle_get_connections(&self) -> Result<serde_json::Value, WireError> {
		let now = Instant::now();
		let connections = self
			.listeners
			.list_connections()
			.into_iter()
			.map(|c| ConnectionInfo {
				conn_id: c.conn_id.to_string(),
				listener_addr: c.listener_addr.to_string(),
				remote: c.remote.to_string(),
				age_ms: u64::try_from(now.saturating_duration_since(c.accepted_at).as_millis())
					.unwrap_or(u64::MAX),
			})
			.collect();
		json(&GetConnectionsResult { listeners: self.listener_status(), connections })
	}

	#[allow(clippy::unused_self)]
	fn handle_get_metrics(&self, args: serde_json::Value) -> Result<serde_json::Value, WireError> {
		let parsed: GetMetricsArgs = serde_json::from_value(args)
			.map_err(|e| WireError { kind: WireErrorKind::BadArgs, message: format!("{e}") })?;
		let format = parsed.format.as_deref().unwrap_or("prometheus");
		let result = match format {
			"" | "prometheus" => {
				let body = vane_engine::metrics::render_prometheus().ok_or_else(|| WireError {
					kind: WireErrorKind::Internal,
					message: "metrics recorder not installed".to_string(),
				})?;
				GetMetricsResult::Prometheus { body }
			}
			"json" => {
				let metrics = vane_engine::metrics::render_json().ok_or_else(|| WireError {
					kind: WireErrorKind::Internal,
					message: "metrics recorder not installed".to_string(),
				})?;
				GetMetricsResult::Json { metrics }
			}
			other => {
				return Err(WireError {
					kind: WireErrorKind::BadArgs,
					message: format!("format must be 'prometheus' or 'json', got {other:?}"),
				});
			}
		};
		serde_json::to_value(result).map_err(|e| WireError {
			kind: WireErrorKind::Internal,
			message: format!("serialize get_metrics result: {e}"),
		})
	}

	fn handle_get_pools(&self) -> Result<serde_json::Value, WireError> {
		let wasm = self
			.wasm_pool_stats
			.as_ref()
			.map(|h| h.snapshot())
			.unwrap_or_default()
			.into_iter()
			.map(|s| WasmPoolEntry {
				kind: s.kind,
				key: s.key,
				export: s.export,
				capacity: s.capacity,
				available: s.available,
				in_use: s.capacity.saturating_sub(s.available),
				total_allocations: s.total_allocations,
				failures: s.failures,
			})
			.collect();
		let cgi = cgi_pool_entry();
		json(&GetPoolsResult { wasm, cgi })
	}

	#[allow(clippy::unused_self)]
	fn handle_get_upstreams(&self) -> Result<serde_json::Value, WireError> {
		let tcp = vane_engine::fetch::client_cache::snapshot()
			.into_iter()
			.map(|s| TcpUpstreamEntry {
				version: s.version.to_string(),
				scheme: s.scheme.to_string(),
				root_ca: s.root_ca.to_string(),
				verify_mode: s.verify_mode.to_string(),
				alpn: s.alpn,
				dns: s.dns.to_string(),
				fingerprint_id: s.fingerprint_id,
			})
			.collect();
		let quic = quic_upstream_entries();
		json(&GetUpstreamsResult { tcp, quic })
	}

	/// Manual pool eviction. Operators read `fingerprint_id` from
	/// `get_upstreams` and pass it back to drain matching cache
	/// entries. Live `Arc<Client>` references survive — only future
	/// cache lookups are affected (per spec § _Lifetime: daemon-level_
	/// drain semantics).
	fn handle_pool_drain(args: serde_json::Value) -> Result<serde_json::Value, WireError> {
		let parsed: vane_mgmt::verb::PoolDrainArgs = serde_json::from_value(args).map_err(|e| {
			WireError { kind: WireErrorKind::BadArgs, message: format!("pool_drain args: {e}") }
		})?;
		let id = parsed.fingerprint_id.trim();
		if id.is_empty() {
			return Err(WireError {
				kind: WireErrorKind::BadArgs,
				message: "pool_drain: fingerprint_id must not be empty".to_string(),
			});
		}
		let tcp_drained = vane_engine::fetch::client_cache::drain_by_fingerprint_id(id);
		let quic_drained = quic_drain_by_id(id);
		json(&vane_mgmt::verb::PoolDrainResult { tcp_drained, quic_drained })
	}

	/// `force_renew` verb: kick off an immediate renewal attempt for
	/// `sni`, bypassing both the periodic timer and any active
	/// backoff. Per `spec/crates/engine-acme.md` § _force_renew mgmt verb_ the
	/// response shape is `{queued, current_status}`; the actual
	/// issuance runs asynchronously, so `queued: true` is "request
	/// accepted" — operators chain a `get_certs` poll if they need
	/// to confirm the cert landed.
	#[cfg(feature = "acme")]
	fn handle_force_renew(&self, args: serde_json::Value) -> Result<serde_json::Value, WireError> {
		use vane_mgmt::verb::{ForceRenewArgs, ForceRenewResult};

		let parsed: ForceRenewArgs = serde_json::from_value(args).map_err(|e| WireError {
			kind: WireErrorKind::BadArgs,
			message: format!("force_renew args: {e}"),
		})?;
		let sni = parsed.sni.trim();
		if sni.is_empty() {
			return Err(WireError {
				kind: WireErrorKind::BadArgs,
				message: "force_renew: sni must not be empty".to_owned(),
			});
		}
		let registry = match self.acme_registry.as_ref() {
			Some(r) => Arc::clone(r),
			None => {
				// daemon was built with `acme` but the current config has
				// no managed certs at all → registry was never opened.
				return json(&ForceRenewResult { queued: false, current_status: "unknown".into() });
			}
		};

		// Look up current status before dispatching so the response
		// reflects the pre-spawn state — the spawned task will mutate
		// it asynchronously.
		let current_status =
			registry.cert_state(sni).map_or_else(|| "unknown".to_owned(), |state| status_label(&state));

		let job = registry.cert_states_snapshot().into_iter().find(|(s, _)| s == sni).and_then(|_| {
			// Snapshot only carries state, not the job. Dispatching
			// requires the job — fetch it via the registry-internal
			// path. We expose `force_renew_dispatch` for this so the
			// daemon doesn't need to reach into the jobs map directly.
			registry.force_renew(sni)
		});
		let queued = job.is_some();
		json(&ForceRenewResult { queued, current_status })
	}

	/// `get_certs` verb: list every cert the daemon tracks. Managed
	/// certs surface full lifecycle detail (status, attempt
	/// timestamps, last error, ARI window); static certs surface
	/// SNI + `source: "static"` only — operators rotate static
	/// certs by editing rules + reload, so the lifecycle fields are
	/// not meaningful there.
	#[cfg(feature = "acme")]
	fn handle_get_certs(&self) -> Result<serde_json::Value, WireError> {
		use vane_mgmt::verb::{AriWindowWire, CertSummary, GetCertsResult};

		let mut certs: Vec<CertSummary> = Vec::new();

		// Managed certs from the registry.
		if let Some(registry) = self.acme_registry.as_ref() {
			for (sni, state) in registry.cert_states_snapshot() {
				let (not_after, issued_at, ocsp_next_update, ocsp_aia_url, ocsp_status) =
					match &state.stored {
						Some(s) => (
							Some(rfc3339(s.not_after)),
							Some(rfc3339(s.last_renew_at)),
							s.ocsp_next_update.map(rfc3339),
							s.ocsp_aia_url.clone(),
							ocsp_status_label(s.ocsp_response.as_deref(), s.ocsp_aia_url.as_deref()),
						),
						None => (None, None, None, None, "no_staple".to_owned()),
					};
				certs.push(CertSummary {
					sni,
					source: "managed".into(),
					san: Vec::new(),
					not_after,
					issued_at,
					status: status_label(&state),
					last_attempt_at: state.last_attempt_at.map(rfc3339),
					last_error: state.last_error.clone(),
					next_attempt_at: state.next_attempt_at.map(rfc3339),
					ari_window: state
						.ari_window
						.as_ref()
						.map(|w| AriWindowWire { start: rfc3339(w.start), end: rfc3339(w.end) }),
					ocsp_status,
					ocsp_next_update,
					ocsp_aia_url,
				});
			}
		}

		// Static certs from the active graph's listener TLS specs.
		// We surface SNI + source only — reading the PEMs to extract
		// not_after / SANs would re-do the work the static populator
		// already did at link time, and the rotation lifecycle for
		// static certs is "edit + reload" which doesn't surface
		// useful per-cert state through this verb.
		let graph = self.graph_swap.load();
		let mut static_snis: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
		for spec in graph.symbolic().meta.listener_tls.values() {
			if let Some(default) = &spec.default
				&& default.is_static()
			{
				static_snis.insert("<default>".to_owned());
			}
			for sni in spec.sni_certs.keys() {
				static_snis.insert(sni.clone());
			}
		}
		for sni in static_snis {
			certs.push(CertSummary {
				sni,
				source: "static".into(),
				san: Vec::new(),
				not_after: None,
				issued_at: None,
				status: String::new(),
				last_attempt_at: None,
				last_error: None,
				next_attempt_at: None,
				ari_window: None,
				// Static certs don't surface OCSP status through
				// this verb — the static populator's OCSP cache is
				// listener-side, not registry-side, and inventorying
				// it would mean walking every listener's `ArcSwap`
				// `CertStore`. Operators check static-cert OCSP
				// health via the underlying populator's logs.
				ocsp_status: String::new(),
				ocsp_next_update: None,
				ocsp_aia_url: None,
			});
		}

		json(&GetCertsResult { certs })
	}

	/// Walk the active graph's `entries` and report each listener's
	/// runtime status. Used by both `stats` and `get_connections` —
	/// they currently return the same per-listener shape; per-connection
	/// detail lands in a later chunk once the listener set registers
	/// `ConnContext`s.
	fn listener_status(&self) -> Vec<ListenerStatus> {
		let graph = self.graph_swap.load();
		graph
			.symbolic()
			.entries
			.keys()
			.map(|addr| ListenerStatus {
				addr: addr.to_string(),
				bound: self.listeners.is_bound(addr),
				in_flight_count: self.listeners.in_flight_count(addr).unwrap_or(0),
			})
			.collect()
	}
}

#[cfg(test)]
mod tests {
	use std::fs;

	use vane_engine::fetch::{http_proxy, http_synthesize, l4_forward};
	use vane_engine::middleware::{forward_client_ip, host_header_match, method_match, path_prefix};

	use super::*;

	struct NullSink;
	impl vane_core::FlowLogSink for NullSink {
		fn emit(&self, _event: vane_core::FlowLogEvent) {}
	}

	fn build_factories() -> (Arc<MiddlewareFactories>, Arc<FetchFactories>) {
		let mut mw = MiddlewareFactories::new();
		host_header_match::register(&mut mw);
		path_prefix::register(&mut mw);
		method_match::register(&mut mw);
		forward_client_ip::register(&mut mw);
		let mut fetch = FetchFactories::new();
		l4_forward::register(&mut fetch);
		http_proxy::register(&mut fetch, None);
		http_synthesize::register(&mut fetch);
		(Arc::new(mw), Arc::new(fetch))
	}

	fn rule(port: u16, body: &str) -> String {
		format!(
			r#"{{
				"rules": [{{
					"preset": "static_site",
					"name": "site",
					"listen": ["127.0.0.1:{port}"],
					"args": {{ "status": 200, "body": "{body}" }}
				}}]
			}}"#
		)
	}

	/// Drive `dispatch` and assert the outcome was a one-shot, returning
	/// the inner result. Streaming verbs are unwrapped separately by
	/// the dedicated `tail_flow` test below.
	async fn one_shot(state: &MgmtState, req: Request) -> Result<serde_json::Value, WireError> {
		match state.dispatch(req).await {
			DispatchOutcome::OneShot(r) => r,
			DispatchOutcome::Stream(_) => panic!("expected OneShot, got Stream"),
		}
	}

	fn initial_state(tmp: &tempfile::TempDir, port: u16) -> Arc<MgmtState> {
		fs::create_dir(tmp.path().join("rules")).unwrap();
		fs::write(tmp.path().join("rules").join("site.json"), rule(port, "v1")).unwrap();

		let loaded = vane_core::config::load(tmp.path()).expect("load");
		let providers = MetadataProviders::new();
		let symbolic = compile(loaded.files, &providers, &providers).expect("compile");
		let (mw, fetch) = build_factories();
		let graph = FlowGraph::link(symbolic, &mw, &fetch).expect("link");
		let swap = Arc::new(ArcSwap::new(graph));

		Arc::new(MgmtState {
			started_at: Instant::now(),
			graph_swap: swap,
			listeners: Arc::new(ListenerSet::new()),
			mw_factories: mw,
			fetch_factories: fetch,
			config_dir: tmp.path().to_path_buf(),
			verbosity: Arc::new(VerbosityState::new()),
			log_sink: Arc::new(NullSink),
			broadcast: Arc::new(BroadcastSink::new()),
			tracing_broadcast: BroadcastTracingLayer::new(),
			security_cfg: Arc::new(SecurityConfig::default()),
			shutdown_trigger: CancellationToken::new(),
			wasm_pool_stats: None,
			plugin_registry: None,
			#[cfg(feature = "wasm")]
			plugin_policies: None,
			#[cfg(feature = "wasm")]
			wasm_runtime: None,
			#[cfg(feature = "wasm")]
			wasm_dir: tmp.path().join("wasm"),
			#[cfg(feature = "acme")]
			acme_registry: None,
		})
	}

	#[tokio::test]
	async fn dispatch_unknown_verb_returns_unknown_verb_error() {
		let tmp = tempfile::tempdir().unwrap();
		let state = initial_state(&tmp, 41001);
		let err =
			one_shot(&state, Request { id: 1, verb: "wat".to_string(), args: serde_json::Value::Null })
				.await
				.expect_err("must error");
		assert_eq!(err.kind, WireErrorKind::UnknownVerb);
	}

	#[tokio::test]
	async fn dispatch_ping_returns_pong_with_version() {
		let tmp = tempfile::tempdir().unwrap();
		let state = initial_state(&tmp, 41002);
		let value =
			one_shot(&state, Request { id: 1, verb: VERB_PING.into(), args: serde_json::Value::Null })
				.await
				.expect("ok");
		let r: PingResult = serde_json::from_value(value).expect("decode");
		assert!(r.pong);
		assert_eq!(r.version, env!("CARGO_PKG_VERSION"));
	}

	#[tokio::test]
	async fn dispatch_stats_includes_listener_addresses_from_graph() {
		let tmp = tempfile::tempdir().unwrap();
		let state = initial_state(&tmp, 41003);
		let value =
			one_shot(&state, Request { id: 1, verb: VERB_STATS.into(), args: serde_json::Value::Null })
				.await
				.expect("ok");
		let r: StatsResult = serde_json::from_value(value).expect("decode");
		assert_eq!(r.graph_version_hash.len(), 64, "hash hex must be 64 chars");
		assert_eq!(r.listeners.len(), 1);
		assert_eq!(r.listeners[0].addr, "127.0.0.1:41003");
		// Listener set never started in tests, so bound=false and counts are zero.
		assert!(!r.listeners[0].bound);
		assert_eq!(r.listeners[0].in_flight_count, 0);
		// No tail_flow / tail_log subscribers in this fixture, so the
		// new subscriber counts default to 0.
		assert_eq!(r.flow_log_subscribers, 0);
		assert_eq!(r.tracing_log_subscribers, 0);
	}

	#[tokio::test]
	async fn dispatch_shutdown_fires_trigger() {
		let tmp = tempfile::tempdir().unwrap();
		let state = initial_state(&tmp, 41004);
		assert!(!state.shutdown_trigger.is_cancelled());
		let value = one_shot(
			&state,
			Request { id: 1, verb: VERB_SHUTDOWN.into(), args: serde_json::Value::Null },
		)
		.await
		.expect("ok");
		let r: ShutdownResult = serde_json::from_value(value).expect("decode");
		assert!(r.draining);
		assert!(state.shutdown_trigger.is_cancelled(), "trigger fired");
	}

	#[tokio::test]
	async fn dispatch_reload_returns_unchanged_on_noop_reload() {
		let tmp = tempfile::tempdir().unwrap();
		let state = initial_state(&tmp, 41005);
		let h0 = state.graph_swap.load().meta().version_hash;
		let value =
			one_shot(&state, Request { id: 1, verb: VERB_RELOAD.into(), args: serde_json::Value::Null })
				.await
				.expect("ok");
		let r: ReloadResult = serde_json::from_value(value).expect("decode");
		match r {
			ReloadResult::Unchanged { hash } => assert_eq!(hash, hex32(&h0)),
			ReloadResult::Swapped { .. } => panic!("expected Unchanged for byte-identical config"),
		}
	}

	#[tokio::test]
	async fn dispatch_reload_swaps_when_rule_body_changes() {
		let tmp = tempfile::tempdir().unwrap();
		let state = initial_state(&tmp, 41006);
		let h0 = state.graph_swap.load().meta().version_hash;
		// Rewrite with a different body.
		fs::write(tmp.path().join("rules").join("site.json"), rule(41006, "v2")).unwrap();

		let value =
			one_shot(&state, Request { id: 1, verb: VERB_RELOAD.into(), args: serde_json::Value::Null })
				.await
				.expect("ok");
		let r: ReloadResult = serde_json::from_value(value).expect("decode");
		match r {
			ReloadResult::Swapped { hash } => {
				assert_ne!(hash, hex32(&h0));
				assert_eq!(state.graph_swap.load().meta().version_hash.to_vec().len(), 32);
			}
			ReloadResult::Unchanged { .. } => panic!("expected Swapped after body change"),
		}
	}

	#[tokio::test]
	async fn dispatch_compile_dry_run_runs_pipeline_against_arg_dir() {
		let tmp_a = tempfile::tempdir().unwrap();
		let state = initial_state(&tmp_a, 41007);
		let h0 = state.graph_swap.load().meta().version_hash;

		// Build a separate config directory with a different rule body.
		let tmp_b = tempfile::tempdir().unwrap();
		fs::create_dir(tmp_b.path().join("rules")).unwrap();
		fs::write(tmp_b.path().join("rules").join("site.json"), rule(41008, "different")).unwrap();

		let args = serde_json::to_value(CompileDryRunArgs {
			config_dir: tmp_b.path().to_string_lossy().into_owned(),
		})
		.unwrap();
		let value = one_shot(&state, Request { id: 1, verb: VERB_COMPILE_DRY_RUN.into(), args })
			.await
			.expect("ok");
		let r: CompileDryRunResult = serde_json::from_value(value).expect("decode");
		assert!(r.graph.is_object(), "graph payload is a JSON object");
		assert!(r.graph.get("entries").is_some(), "symbolic graph carries `entries`");
		// Active graph must be untouched.
		assert_eq!(state.graph_swap.load().meta().version_hash, h0);
	}

	#[tokio::test]
	async fn dispatch_get_config_returns_symbolic_graph() {
		let tmp = tempfile::tempdir().unwrap();
		let state = initial_state(&tmp, 41009);
		let value = one_shot(
			&state,
			Request { id: 1, verb: VERB_GET_CONFIG.into(), args: serde_json::Value::Null },
		)
		.await
		.expect("ok");
		let r: GetConfigResult = serde_json::from_value(value).expect("decode");
		assert!(r.graph.get("entries").is_some());
		assert!(r.graph.get("nodes").is_some());
		assert!(r.graph.get("meta").is_some());
	}

	#[tokio::test]
	async fn dispatch_get_connections_returns_per_listener_summary() {
		let tmp = tempfile::tempdir().unwrap();
		let state = initial_state(&tmp, 41010);
		let value = one_shot(
			&state,
			Request { id: 1, verb: VERB_GET_CONNECTIONS.into(), args: serde_json::Value::Null },
		)
		.await
		.expect("ok");
		let r: GetConnectionsResult = serde_json::from_value(value).expect("decode");
		assert_eq!(r.listeners.len(), 1);
		assert_eq!(r.listeners[0].addr, "127.0.0.1:41010");
	}

	#[tokio::test]
	async fn dispatch_compile_dry_run_bad_args_kind_is_bad_args() {
		let tmp = tempfile::tempdir().unwrap();
		let state = initial_state(&tmp, 41011);
		let err = one_shot(
			&state,
			Request {
				id: 1,
				verb: VERB_COMPILE_DRY_RUN.into(),
				// Missing `config_dir` key.
				args: serde_json::json!({}),
			},
		)
		.await
		.expect_err("must error");
		assert_eq!(err.kind, WireErrorKind::BadArgs);
	}

	#[tokio::test]
	async fn dispatch_tail_flow_returns_stream_that_yields_emitted_events() {
		use vane_core::{ConnId, FlowLogEvent, FlowLogKind};

		let tmp = tempfile::tempdir().unwrap();
		let state = initial_state(&tmp, 41012);

		// Pull a Stream out of the dispatcher.
		let outcome = state
			.dispatch(Request { id: 1, verb: VERB_TAIL_FLOW.into(), args: serde_json::Value::Null })
			.await;
		let mut stream = match outcome {
			DispatchOutcome::Stream(s) => s,
			DispatchOutcome::OneShot(_) => panic!("tail_flow must produce a Stream"),
		};

		// Emit a FlowLogEvent through the broadcast sink and observe it
		// pop out as a wire-shape JSON object on the stream.
		let evt = FlowLogEvent {
			t: 0,
			conn: ConnId(0xFEED),
			seq: 0,
			kind: FlowLogKind::Trajectory,
			node: None,
			error: None,
			data: None,
		};
		<BroadcastSink as FlowLogSink>::emit(&state.broadcast, evt);
		let value = tokio::time::timeout(std::time::Duration::from_secs(1), stream.next_event())
			.await
			.expect("event arrives within 1s")
			.expect("stream still open");
		assert_eq!(value["kind"], "Trajectory");
		assert_eq!(value["conn"], 0xFEED);
	}

	#[tokio::test]
	async fn dispatch_get_pools_returns_empty_wasm_when_runtime_absent() {
		let tmp = tempfile::tempdir().unwrap();
		let state = initial_state(&tmp, 41013);
		let value = one_shot(
			&state,
			Request { id: 1, verb: VERB_GET_POOLS.into(), args: serde_json::Value::Null },
		)
		.await
		.expect("ok");
		let r: GetPoolsResult = serde_json::from_value(value).expect("decode");
		assert!(r.wasm.is_empty(), "no WasmPoolStats plumbed in this fixture");
		// CGI pool: present iff the engine semaphore has been
		// initialised by some sibling test in this binary. The shape is
		// what we lock here — None is acceptable, Some must validate.
		if let Some(cgi) = r.cgi {
			assert!(cgi.cap > 0);
			assert_eq!(cgi.cap, cgi.available + cgi.in_use);
		}
	}

	#[tokio::test]
	async fn dispatch_get_upstreams_returns_known_shape() {
		let tmp = tempfile::tempdir().unwrap();
		let state = initial_state(&tmp, 41014);
		let value = one_shot(
			&state,
			Request { id: 1, verb: VERB_GET_UPSTREAMS.into(), args: serde_json::Value::Null },
		)
		.await
		.expect("ok");
		let r: GetUpstreamsResult = serde_json::from_value(value).expect("decode");
		// The client_cache + quic_pool are process-wide statics so other
		// tests in this binary may have populated them. We only lock
		// the fact that `tcp` decodes as a list and each entry is well-
		// formed; cardinality is not asserted to keep the test stable
		// under parallel scheduling.
		for entry in &r.tcp {
			assert!(matches!(entry.scheme.as_str(), "http" | "https"));
		}
		// QUIC list is empty unless a successful dial happened earlier
		// in this binary; either way the field must decode.
		let _ = r.quic;
	}

	#[tokio::test]
	async fn dispatch_get_pools_includes_wasm_entries_via_stub() {
		// Plumb a WasmPoolStats stub so the dispatcher's wasm branch
		// surfaces non-empty entries with the expected `in_use`
		// arithmetic.
		struct Stub;
		impl WasmPoolStats for Stub {
			fn snapshot(&self) -> Vec<vane_core::WasmPoolSummary> {
				vec![vane_core::WasmPoolSummary {
					kind: "stateful".to_string(),
					key: "/etc/vaned/plugins/edge.wasm".to_string(),
					export: "l4-peek".to_string(),
					capacity: 4,
					available: 1,
					total_allocations: 17,
					failures: 2,
				}]
			}
		}

		let tmp = tempfile::tempdir().unwrap();
		let mut state = initial_state(&tmp, 41015);
		// `Arc::get_mut` succeeds because no other clone of `state`
		// exists at this point in the test.
		Arc::get_mut(&mut state).expect("unique Arc").wasm_pool_stats =
			Some(Arc::new(Stub) as Arc<dyn WasmPoolStats>);
		let value = one_shot(
			&state,
			Request { id: 1, verb: VERB_GET_POOLS.into(), args: serde_json::Value::Null },
		)
		.await
		.expect("ok");
		let r: GetPoolsResult = serde_json::from_value(value).expect("decode");
		assert_eq!(r.wasm.len(), 1);
		let entry = &r.wasm[0];
		assert_eq!(entry.kind, "stateful");
		assert_eq!(entry.export, "l4-peek");
		assert_eq!(entry.capacity, 4);
		assert_eq!(entry.available, 1);
		assert_eq!(entry.in_use, 3, "in_use must be capacity - available");
		assert_eq!(entry.total_allocations, 17, "counters surface from the stub");
		assert_eq!(entry.failures, 2, "failures surface from the stub");
	}

	#[tokio::test]
	async fn dispatch_get_upstreams_lists_tcp_after_factory_call() {
		// Drive the http_proxy factory with a cleartext upstream so the
		// client_cache picks up a known fingerprint without us having
		// to construct a TLS connector by hand. The dispatcher must
		// surface that fingerprint among its TCP entries.
		vane_engine::crypto::install_default_provider();
		let _ = vane_engine::fetch::http_proxy::factory(
			&serde_json::json!({
				"upstream": "127.0.0.1:9999",
				"version": "h1",
			}),
			None,
		)
		.expect("cleartext h1 factory must succeed");

		let tmp = tempfile::tempdir().unwrap();
		let state = initial_state(&tmp, 41016);
		let value = one_shot(
			&state,
			Request { id: 1, verb: VERB_GET_UPSTREAMS.into(), args: serde_json::Value::Null },
		)
		.await
		.expect("ok");
		let r: GetUpstreamsResult = serde_json::from_value(value).expect("decode");
		assert!(
			r.tcp.iter().any(|e| e.version == "h1"
				&& e.scheme == "http"
				&& e.root_ca == "none"
				&& e.dns == "system"),
			"the cleartext h1 fingerprint we built must surface in the snapshot",
		);
		assert!(
			r.tcp.iter().all(|e| !e.fingerprint_id.is_empty()),
			"every entry has a non-empty fingerprint_id",
		);
	}

	#[tokio::test]
	async fn dispatch_pool_drain_removes_matching_entry() {
		vane_engine::crypto::install_default_provider();
		// Build a unique fingerprint for this test, then drain by id.
		let _ = vane_engine::fetch::http_proxy::factory(
			&serde_json::json!({ "upstream": "127.0.0.1:9998", "version": "h2" }),
			None,
		)
		.expect("h2 factory must succeed");

		let tmp = tempfile::tempdir().unwrap();
		let state = initial_state(&tmp, 41017);
		let value = one_shot(
			&state,
			Request { id: 1, verb: VERB_GET_UPSTREAMS.into(), args: serde_json::Value::Null },
		)
		.await
		.expect("ok");
		let r: GetUpstreamsResult = serde_json::from_value(value).expect("decode");
		let entry = r.tcp.iter().find(|e| e.version == "h2").expect("h2 entry present");
		let id = entry.fingerprint_id.clone();
		assert!(!id.is_empty());

		let drain = one_shot(
			&state,
			Request {
				id: 2,
				verb: vane_mgmt::verb::VERB_POOL_DRAIN.into(),
				args: serde_json::json!({ "fingerprint_id": id }),
			},
		)
		.await
		.expect("drain ok");
		let r: vane_mgmt::verb::PoolDrainResult = serde_json::from_value(drain).expect("decode drain");
		assert_eq!(r.tcp_drained, 1, "exactly one tcp entry drained");
		assert_eq!(r.quic_drained, 0, "no quic entries drained for a tcp-only id");
	}

	#[tokio::test]
	async fn dispatch_pool_drain_rejects_empty_id() {
		let tmp = tempfile::tempdir().unwrap();
		let state = initial_state(&tmp, 41018);
		let err = one_shot(
			&state,
			Request {
				id: 1,
				verb: vane_mgmt::verb::VERB_POOL_DRAIN.into(),
				args: serde_json::json!({ "fingerprint_id": "" }),
			},
		)
		.await
		.expect_err("empty id must fail");
		assert!(matches!(err.kind, vane_mgmt::WireErrorKind::BadArgs));
	}

	#[cfg(feature = "acme")]
	#[tokio::test]
	async fn dispatch_force_renew_unknown_sni_when_no_registry() {
		// When the daemon has no acme_registry (no managed rules in
		// the active config), force_renew returns queued=false +
		// status="unknown" rather than failing as an unknown verb.
		let tmp = tempfile::tempdir().unwrap();
		let state = initial_state(&tmp, 41021);
		let value = one_shot(
			&state,
			Request {
				id: 1,
				verb: VERB_FORCE_RENEW.into(),
				args: serde_json::json!({ "sni": "api.example.com" }),
			},
		)
		.await
		.expect("ok");
		let r: vane_mgmt::verb::ForceRenewResult = serde_json::from_value(value).expect("decode");
		assert!(!r.queued);
		assert_eq!(r.current_status, "unknown");
	}

	#[cfg(feature = "acme")]
	#[tokio::test]
	async fn dispatch_force_renew_rejects_empty_sni() {
		let tmp = tempfile::tempdir().unwrap();
		let state = initial_state(&tmp, 41022);
		let err = one_shot(
			&state,
			Request { id: 1, verb: VERB_FORCE_RENEW.into(), args: serde_json::json!({ "sni": "" }) },
		)
		.await
		.expect_err("empty sni must fail");
		assert!(matches!(err.kind, vane_mgmt::WireErrorKind::BadArgs));
	}

	#[cfg(feature = "acme")]
	#[tokio::test]
	async fn dispatch_get_certs_returns_empty_when_no_managed_or_static() {
		let tmp = tempfile::tempdir().unwrap();
		let state = initial_state(&tmp, 41023);
		let value = one_shot(
			&state,
			Request { id: 1, verb: VERB_GET_CERTS.into(), args: serde_json::Value::Null },
		)
		.await
		.expect("ok");
		let r: vane_mgmt::verb::GetCertsResult = serde_json::from_value(value).expect("decode");
		assert!(r.certs.is_empty(), "fixture graph has no tls listeners");
	}

	#[cfg(feature = "acme")]
	#[tokio::test]
	async fn dispatch_force_renew_with_registry_queues_when_job_registered() {
		use std::sync::Arc;

		use vane_engine::acme::{ManagedCertRegistry, RenewalJob};

		let tmp = tempfile::tempdir().unwrap();
		let mut state = initial_state(&tmp, 41024);
		// Open an in-memory registry; register a job for the SNI.
		// Use the FsAcmeStore on a tmpdir so the lock + persistence
		// pieces don't need mocking here.
		let acme_dir = tmp.path().join("acme");
		std::fs::create_dir_all(&acme_dir).unwrap();
		let store = vane_engine::acme::FsAcmeStore::open(&acme_dir).expect("fs store");
		let registry = ManagedCertRegistry::open(Arc::new(store)).await.expect("open registry");
		let _ = registry.declare_managed(&["api.example.com".into()]);
		registry.register_renewal_job(
			"api.example.com",
			RenewalJob {
				directory_url: "https://acme.invalid/dir".into(),
				contact: vec!["mailto:ops@example.com".into()],
				challenge: vane_core::rule::ChallengeKind::Http01,
				dns: None,
				renew_before: std::time::Duration::from_secs(30 * 24 * 60 * 60),
				extra_root_ca_pem: None,
			},
		);
		// Inject the registry into the test fixture's MgmtState.
		Arc::get_mut(&mut state).expect("unique state").acme_registry.replace(Arc::clone(&registry));

		let value = one_shot(
			&state,
			Request {
				id: 1,
				verb: VERB_FORCE_RENEW.into(),
				args: serde_json::json!({ "sni": "api.example.com" }),
			},
		)
		.await
		.expect("ok");
		let r: vane_mgmt::verb::ForceRenewResult = serde_json::from_value(value).expect("decode");
		assert!(r.queued, "registered job → queued");
		// Status was "valid" (fresh state default) at the moment of
		// the call; the spawn races forward asynchronously, but the
		// returned status reflects pre-spawn state.
		assert_eq!(r.current_status, "valid");
	}

	#[cfg(feature = "acme")]
	#[test]
	fn ocsp_status_label_branches() {
		// stapled: bytes present.
		assert_eq!(ocsp_status_label(Some(&[0u8; 4]), Some("http://x")), "stapled");
		assert_eq!(ocsp_status_label(Some(&[0u8; 4]), None), "stapled");
		// fetch_failed: AIA URL known but no staple cached yet.
		assert_eq!(ocsp_status_label(None, Some("http://x")), "fetch_failed");
		// no_staple: cert has no AIA URL at all.
		assert_eq!(ocsp_status_label(None, None), "no_staple");
	}

	#[cfg(feature = "acme")]
	#[tokio::test]
	async fn dispatch_get_certs_surfaces_ocsp_fields_for_managed_cert() {
		use std::sync::Arc;
		use std::time::Duration;

		use async_trait::async_trait;
		use parking_lot::Mutex;
		use vane_engine::acme::{
			AcmeAccount, AcmeStore, LockGuard, ManagedCertRegistry, StoreError, StoredCert,
		};

		// Mock store that hydrates `api.example.com` with a cert
		// that already carries OCSP fields. Avoids needing
		// FsAcmeStore + on-disk cert files for what is really a
		// wire-shape test.
		#[derive(Default)]
		struct MockStore {
			certs: Mutex<std::collections::BTreeMap<String, StoredCert>>,
		}
		#[derive(Debug)]
		struct MockGuard;
		impl LockGuard for MockGuard {}
		#[async_trait]
		impl AcmeStore for MockStore {
			async fn load_account(&self, _: &str) -> Result<Option<AcmeAccount>, StoreError> {
				Ok(None)
			}
			async fn save_account(&self, _: &str, _: &AcmeAccount) -> Result<(), StoreError> {
				Ok(())
			}
			async fn load_cert(&self, sni: &str) -> Result<Option<StoredCert>, StoreError> {
				Ok(self.certs.lock().get(sni).cloned())
			}
			async fn save_cert(&self, sni: &str, cert: &StoredCert) -> Result<(), StoreError> {
				self.certs.lock().insert(sni.to_owned(), cert.clone());
				Ok(())
			}
			async fn list_cert_snis(&self) -> Result<Vec<String>, StoreError> {
				Ok(self.certs.lock().keys().cloned().collect())
			}
			async fn lock(&self, _: &str) -> Result<Box<dyn LockGuard>, StoreError> {
				Ok(Box::new(MockGuard))
			}
		}

		let store = Arc::new(MockStore::default());
		let stored = StoredCert {
			leaf_pem: "-----BEGIN CERTIFICATE-----\nLEAF\n-----END CERTIFICATE-----\n".into(),
			chain_pem: String::new(),
			key_pem: "-----BEGIN PRIVATE KEY-----\nKEY\n-----END PRIVATE KEY-----\n".into(),
			not_after: std::time::SystemTime::UNIX_EPOCH + Duration::from_hours(500_000),
			ari_replacement_id: None,
			last_renew_at: std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
			ocsp_response: Some(b"DER".to_vec()),
			ocsp_next_update: Some(std::time::SystemTime::UNIX_EPOCH + Duration::from_hours(500_000)),
			ocsp_aia_url: Some("http://ocsp.example.test/".into()),
		};
		store.save_cert("api.example.com", &stored).await.unwrap();
		let registry = ManagedCertRegistry::open(store as Arc<dyn AcmeStore>).await.expect("open");

		let tmp = tempfile::tempdir().unwrap();
		let mut state = initial_state(&tmp, 41025);
		Arc::get_mut(&mut state).expect("unique").acme_registry.replace(Arc::clone(&registry));

		let value = one_shot(
			&state,
			Request { id: 1, verb: VERB_GET_CERTS.into(), args: serde_json::Value::Null },
		)
		.await
		.expect("ok");
		let r: vane_mgmt::verb::GetCertsResult = serde_json::from_value(value).expect("decode");
		let entry = r.certs.iter().find(|c| c.sni == "api.example.com").expect("cert listed");
		assert_eq!(entry.ocsp_status, "stapled");
		assert_eq!(entry.ocsp_aia_url.as_deref(), Some("http://ocsp.example.test/"));
		assert!(entry.ocsp_next_update.is_some());
	}
}
