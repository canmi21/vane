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
use vane_core::{FlowLogEvent, FlowLogSink};
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
	CompileDryRunArgs, CompileDryRunResult, ConnectionInfo, GetActiveConfigResult,
	ListConnectionsResult, ListenerStatus, PingResult, ReloadResult, ShutdownResult, StatsResult,
	VERB_COMPILE_DRY_RUN, VERB_GET_ACTIVE_CONFIG, VERB_LIST_CONNECTIONS, VERB_PING, VERB_RELOAD,
	VERB_SHUTDOWN, VERB_STATS, VERB_TAIL_FLOW_LOG, VERB_TAIL_LOG,
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
	/// Live broadcast handle. `tail_flow_log` subscribes here for
	/// incident-time event streaming.
	pub broadcast: Arc<BroadcastSink>,
	/// Tracing layer that broadcasts every emitted event. `tail_log`
	/// subscribes here. Cheap to clone (wraps a [`broadcast::Sender`]).
	pub tracing_broadcast: BroadcastTracingLayer,
	pub security_cfg: Arc<SecurityConfig>,
	/// Fired by the `shutdown` verb. The daemon's main signal loop
	/// awaits this alongside SIGINT/SIGTERM.
	pub shutdown_trigger: CancellationToken,
}

#[async_trait]
impl Handler for MgmtState {
	async fn dispatch(&self, req: Request) -> DispatchOutcome {
		// Streaming verbs are dispatched first because their return type
		// is `Stream`, not `OneShot`. Everything else funnels through the
		// shared one-shot path below.
		if req.verb == VERB_TAIL_FLOW_LOG {
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
			VERB_GET_ACTIVE_CONFIG => self.handle_get_active_config(),
			VERB_RELOAD => self.handle_reload(),
			VERB_COMPILE_DRY_RUN => self.handle_compile_dry_run(req.args),
			VERB_LIST_CONNECTIONS => self.handle_list_connections(),
			other => Err(WireError {
				kind: WireErrorKind::UnknownVerb,
				message: format!("unknown verb {other:?}"),
			}),
		};
		DispatchOutcome::OneShot(result)
	}
}

/// Streaming source for the `tail_flow_log` verb. Wraps a per-call
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
					tracing::warn!(dropped = n, "tail_flow_log subscriber lagged");
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

fn json<R: serde::Serialize>(r: &R) -> Result<serde_json::Value, WireError> {
	serde_json::to_value(r)
		.map_err(|e| WireError { kind: WireErrorKind::Internal, message: format!("encode: {e}") })
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
		})
	}

	fn handle_shutdown(&self) -> Result<serde_json::Value, WireError> {
		self.shutdown_trigger.cancel();
		json(&ShutdownResult { draining: true })
	}

	fn handle_get_active_config(&self) -> Result<serde_json::Value, WireError> {
		let graph = self.graph_swap.load();
		let serialized = serde_json::to_value(graph.symbolic().as_ref()).map_err(|e| WireError {
			kind: WireErrorKind::Internal,
			message: format!("symbolic: {e}"),
		})?;
		json(&GetActiveConfigResult { graph: serialized })
	}

	fn handle_reload(&self) -> Result<serde_json::Value, WireError> {
		match reload_once(
			&self.config_dir,
			&self.graph_swap,
			&self.mw_factories,
			&self.fetch_factories,
			&self.security_cfg,
		) {
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

	#[allow(clippy::unused_self)]
	fn handle_compile_dry_run(
		&self,
		args: serde_json::Value,
	) -> Result<serde_json::Value, WireError> {
		let args: CompileDryRunArgs = parse_args(args)?;
		let dir = PathBuf::from(args.config_dir);
		let loaded = vane_core::config::load(&dir)
			.map_err(|e| WireError { kind: WireErrorKind::BadArgs, message: format!("load: {e}") })?;
		let providers = MetadataProviders;
		let symbolic = compile(loaded.files, &providers, &providers)
			.map_err(|e| WireError { kind: WireErrorKind::BadArgs, message: format!("compile: {e}") })?;
		let value = serde_json::to_value(&symbolic).map_err(|e| WireError {
			kind: WireErrorKind::Internal,
			message: format!("symbolic: {e}"),
		})?;
		json(&CompileDryRunResult { graph: value })
	}

	fn handle_list_connections(&self) -> Result<serde_json::Value, WireError> {
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
		json(&ListConnectionsResult { listeners: self.listener_status(), connections })
	}

	/// Walk the active graph's `entries` and report each listener's
	/// runtime status. Used by both `stats` and `list_connections` —
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
		http_proxy::register(&mut fetch);
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
	/// the dedicated `tail_flow_log` test below.
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
		let providers = MetadataProviders;
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
	async fn dispatch_get_active_config_returns_symbolic_graph() {
		let tmp = tempfile::tempdir().unwrap();
		let state = initial_state(&tmp, 41009);
		let value = one_shot(
			&state,
			Request { id: 1, verb: VERB_GET_ACTIVE_CONFIG.into(), args: serde_json::Value::Null },
		)
		.await
		.expect("ok");
		let r: GetActiveConfigResult = serde_json::from_value(value).expect("decode");
		assert!(r.graph.get("entries").is_some());
		assert!(r.graph.get("nodes").is_some());
		assert!(r.graph.get("meta").is_some());
	}

	#[tokio::test]
	async fn dispatch_list_connections_returns_per_listener_summary() {
		let tmp = tempfile::tempdir().unwrap();
		let state = initial_state(&tmp, 41010);
		let value = one_shot(
			&state,
			Request { id: 1, verb: VERB_LIST_CONNECTIONS.into(), args: serde_json::Value::Null },
		)
		.await
		.expect("ok");
		let r: ListConnectionsResult = serde_json::from_value(value).expect("decode");
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
	async fn dispatch_tail_flow_log_returns_stream_that_yields_emitted_events() {
		use vane_core::{ConnId, FlowLogEvent, FlowLogKind};

		let tmp = tempfile::tempdir().unwrap();
		let state = initial_state(&tmp, 41012);

		// Pull a Stream out of the dispatcher.
		let outcome = state
			.dispatch(Request { id: 1, verb: VERB_TAIL_FLOW_LOG.into(), args: serde_json::Value::Null })
			.await;
		let mut stream = match outcome {
			DispatchOutcome::Stream(s) => s,
			DispatchOutcome::OneShot(_) => panic!("tail_flow_log must produce a Stream"),
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
}
