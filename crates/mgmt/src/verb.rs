//! Per-verb argument and result schemas. Wire-side `Request.args` is a
//! `serde_json::Value` (untyped on the wire); each verb's handler
//! deserialises it into its own typed struct, surfacing
//! [`crate::protocol::WireErrorKind::BadArgs`] on shape mismatch.
//!
//! Verbs: `compile_dry_run`, `reload`, `get_config`, `stats`,
//! `shutdown`, `get_connections`, plus `ping` for cheap liveness checks.
//!
//! See [`spec/crates/mgmt.md` § _Verbs_](../../../spec/crates/mgmt.md#verbs).

use serde::{Deserialize, Serialize};

// ─── Verb names ─────────────────────────────────────────────────────────
pub const VERB_PING: &str = "ping";
pub const VERB_STATS: &str = "stats";
pub const VERB_SHUTDOWN: &str = "shutdown";
pub const VERB_GET_CONFIG: &str = "get_config";
pub const VERB_RELOAD: &str = "reload";
pub const VERB_COMPILE_DRY_RUN: &str = "compile_dry_run";
pub const VERB_GET_CONNECTIONS: &str = "get_connections";
pub const VERB_TAIL_FLOW: &str = "tail_flow";
pub const VERB_TAIL_LOG: &str = "tail_log";
pub const VERB_GET_METRICS: &str = "get_metrics";
pub const VERB_GET_POOLS: &str = "get_pools";
pub const VERB_GET_UPSTREAMS: &str = "get_upstreams";

// ─── Empty args sentinel ────────────────────────────────────────────────
/// Placeholder for verbs that accept no arguments. Round-trips as `{}`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NoArgs {}

// ─── ping ───────────────────────────────────────────────────────────────
/// `ping` is the cheapest liveness verb. The 6 spec verbs all touch
/// daemon state; `ping` only confirms the dispatcher is alive and
/// reports the daemon's build version. Probes / health checks should
/// prefer `ping` over `stats`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PingResult {
	pub pong: bool,
	/// `vaned` `CARGO_PKG_VERSION` at compile time.
	pub version: String,
}

// ─── stats ──────────────────────────────────────────────────────────────
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StatsResult {
	pub uptime_ms: u64,
	/// Lower-case hex of the active flow-graph's SHA-256 version hash.
	pub graph_version_hash: String,
	pub listeners: Vec<ListenerStatus>,
	/// Live `tail_flow` subscribers — `BroadcastSink::subscriber_count`.
	/// Tests use this to wait for streaming readiness rather than
	/// sleeping a fixed interval.
	#[serde(default)]
	pub flow_log_subscribers: usize,
	/// Live `tail_log` subscribers — `BroadcastTracingLayer::subscriber_count`.
	#[serde(default)]
	pub tracing_log_subscribers: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ListenerStatus {
	pub addr: String,
	pub bound: bool,
	pub in_flight_count: usize,
}

// ─── shutdown ───────────────────────────────────────────────────────────
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShutdownResult {
	/// Always `true` on a successful shutdown verb — the daemon has
	/// observed the trigger and is in the soft-drain phase. Operators
	/// should follow up by waiting on the `vaned` process exit.
	pub draining: bool,
}

// ─── get_config ─────────────────────────────────────────────────────────
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetConfigResult {
	/// Serialized `vane_core::SymbolicFlowGraph`. Kept as
	/// `serde_json::Value` so consumers (CLI / TUI / external tools)
	/// don't need to depend on `vane-core` to decode the wire payload.
	pub graph: serde_json::Value,
}

// ─── reload ─────────────────────────────────────────────────────────────
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReloadResult {
	/// Recompile produced a new graph and the runtime swap took effect.
	Swapped { hash: String },
	/// Recompile reproduced the active graph's hash; swap was skipped.
	Unchanged { hash: String },
}

// ─── compile_dry_run ────────────────────────────────────────────────────
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompileDryRunArgs {
	/// Filesystem path to the candidate config tree.
	pub config_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompileDryRunResult {
	/// The compiled (but not linked, not swapped) `SymbolicFlowGraph`.
	pub graph: serde_json::Value,
}

// ─── get_connections ────────────────────────────────────────────────────
/// One in-flight connection on the wire. `conn_id` is hex (16 chars,
/// matches `ConnId`'s `Display`); addresses use the standard
/// `SocketAddr` Display form.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConnectionInfo {
	pub conn_id: String,
	pub listener_addr: String,
	pub remote: String,
	pub age_ms: u64,
}

// ─── get_metrics ────────────────────────────────────────────────────────
/// Args for `get_metrics`. `format` selects the output shape.
///
/// - `"prometheus"` (default, or `null` / missing / `""`) — Prometheus
///   text exposition format.
/// - `"json"` — structured JSON parsed from the text exposition.
/// - Any other value → `WireErrorKind::BadArgs`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GetMetricsArgs {
	/// Output format: `"prometheus"` or `"json"`. Missing / null treated
	/// as `"prometheus"`.
	#[serde(default)]
	pub format: Option<String>,
}

/// Result of `get_metrics`. Tagged by `format` so consumers can branch
/// without an extra discriminant field.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "format", rename_all = "snake_case")]
pub enum GetMetricsResult {
	Prometheus { body: String },
	Json { metrics: serde_json::Value },
}

/// Per-listener summary plus the live in-flight connection list.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GetConnectionsResult {
	pub listeners: Vec<ListenerStatus>,
	#[serde(default)]
	pub connections: Vec<ConnectionInfo>,
}

// ─── get_pools ──────────────────────────────────────────────────────────
/// Snapshot of every daemon-bounded execution pool: WASM stateful /
/// stateless instance pools and the CGI concurrency-cap semaphore.
///
/// Spec § _State_ in `10-management.md` lists the per-pool fields as
/// "pool size, in-use count, total allocations, failures". The first
/// two map directly onto `capacity` / `in_use`; the latter two are
/// reserved on the wire (always `0`) until the daemon plumbs the
/// metrics counters required to populate them.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GetPoolsResult {
	#[serde(default)]
	pub wasm: Vec<WasmPoolEntry>,
	/// `None` when the `cgi` feature is disabled, or when no CGI rule
	/// has fired in this daemon's lifetime (the semaphore is lazily
	/// initialised on the first request).
	#[serde(default)]
	pub cgi: Option<CgiPoolEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WasmPoolEntry {
	/// `"stateful"` or `"stateless"`.
	pub kind: String,
	/// Module identity (canonical absolute path of the `.wasm` file).
	pub key: String,
	/// Plugin export name within the component.
	pub export: String,
	pub capacity: usize,
	pub available: usize,
	pub in_use: usize,
	/// Cumulative successful checkouts (stateful) or rentals
	/// (stateless).
	#[serde(default)]
	pub total_allocations: u64,
	/// Cumulative checkout / instantiation failures.
	#[serde(default)]
	pub failures: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CgiPoolEntry {
	pub cap: usize,
	pub available: usize,
	pub in_use: usize,
	/// Cumulative successful permit acquisitions (CGI fetches that
	/// proceeded to fork/exec).
	#[serde(default)]
	pub total_allocations: u64,
	/// Cumulative cap-rejected fetches (503 fast-rejects, spec
	/// § _Concurrency cap_).
	#[serde(default)]
	pub failures: u64,
}

// ─── get_upstreams ──────────────────────────────────────────────────────
/// Snapshot of cached upstream connection objects: the TCP / TLS
/// `hyper-util` client cache and (when `h3` is built) the QUIC pool.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GetUpstreamsResult {
	#[serde(default)]
	pub tcp: Vec<TcpUpstreamEntry>,
	/// Empty when the `h3` feature is disabled.
	#[serde(default)]
	pub quic: Vec<QuicUpstreamEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TcpUpstreamEntry {
	/// Negotiated upstream version: `"auto"`, `"h1"`, `"h2"`, `"h3"`.
	pub version: String,
	/// `"http"` (cleartext) or `"https"` (TLS).
	pub scheme: String,
	/// Trust-root posture: `"system"`, `"bundle"`, `"insecure-skip"`,
	/// or `"none"` (cleartext).
	pub root_ca: String,
	/// Verify mode: `"full"`, `"skip"`, or `"none"` (cleartext).
	pub verify_mode: String,
	pub alpn: Vec<String>,
	/// `"system"` (read `/etc/resolv.conf`) or `"custom"` (operator-
	/// pinned nameservers).
	pub dns: String,
	/// 16-char hex identifier for `pool.drain`. Stable for the
	/// process lifetime as long as the underlying fingerprint contents
	/// are unchanged.
	#[serde(default)]
	pub fingerprint_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QuicUpstreamEntry {
	pub remote_addr: String,
	pub sni: String,
	pub alpn: Vec<String>,
	/// 16-char hex identifier for `pool.drain`.
	#[serde(default)]
	pub fingerprint_id: String,
}

// ─── pool_drain ─────────────────────────────────────────────────────────
/// Verb name for the manual pool eviction RPC. Operators look up a
/// `fingerprint_id` from `get_upstreams` and pass it back here to
/// remove the matching cache entry. Live `Arc<Client>` /
/// `Arc<QuicPoolEntry>` references survive — only future cache
/// lookups are affected.
pub const VERB_POOL_DRAIN: &str = "pool_drain";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PoolDrainArgs {
	pub fingerprint_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PoolDrainResult {
	/// Number of TCP / TLS `client_cache` entries removed (almost
	/// always 0 or 1).
	pub tcp_drained: usize,
	/// Number of QUIC pool entries removed. `0` when the `h3` feature
	/// is disabled or no QUIC pool entry matches the id.
	pub quic_drained: usize,
}

// ─── force_renew ──────────────────────────────────────────────────────
/// Verb name for the operator-driven "renew this cert NOW" RPC per
/// `spec/acme.md` § _`force_renew` mgmt verb_. Bypasses the
/// `renew_before` timer and any active backoff; useful for
/// key-compromise rotation. The actual issuance runs asynchronously
/// — `queued: true` means the registry accepted the request, not
/// that the cert is in hand.
pub const VERB_FORCE_RENEW: &str = "force_renew";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ForceRenewArgs {
	pub sni: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ForceRenewResult {
	/// `true` when the registry accepted the request and spawned a
	/// renewal task; `false` when the SNI is not declared managed
	/// (no `tls.managed` rule references it) or no renewal job has
	/// been registered for it.
	pub queued: bool,
	/// Cert lifecycle status at the moment the request was received:
	/// `"valid"`, `"renewing"`, `"failed"`, or `"limited"` per spec
	/// § _Rate-limit and failure handling_. `"unknown"` for SNIs
	/// that have never been declared.
	pub current_status: String,
}

// ─── get_certs ──────────────────────────────────────────────────────
/// Verb name for the cert inventory RPC per `spec/acme.md`
/// § _mgmt verbs § `get_certs`_. Lists every cert the daemon tracks —
/// managed (full lifecycle detail) and static (SNI + source label).
pub const VERB_GET_CERTS: &str = "get_certs";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GetCertsResult {
	pub certs: Vec<CertSummary>,
}

/// One cert's wire-shape summary. Field set matches
/// `spec/acme.md` § _`get_certs` response shape_; static-source
/// entries leave the lifecycle fields (`status`, `last_*`,
/// `next_*`, `ari_window`) at their defaults — they're meaningful
/// only for managed certs.
///
/// Named `CertSummary` to disambiguate from
/// `vane_engine::tls::CertEntry` (the rustls-side handshake bundle);
/// the wire shape is operator-facing, the engine type is
/// resolver-internal.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CertSummary {
	pub sni: String,
	/// `"managed"` for ACME-issued certs, `"static"` for operator-
	/// supplied PEMs.
	pub source: String,
	#[serde(default)]
	pub san: Vec<String>,
	/// ISO 8601 / RFC 3339 timestamp. `None` when no cert is
	/// currently issued (managed SNI before first issuance).
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub not_after: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub issued_at: Option<String>,
	/// `"valid"` | `"renewing"` | `"failed"` | `"limited"`. Empty
	/// for static certs.
	#[serde(default)]
	pub status: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub last_attempt_at: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub last_error: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub next_attempt_at: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub ari_window: Option<AriWindowWire>,
	/// OCSP staple status from the cert's perspective:
	///
	/// - `"stapled"`: a fresh OCSP response is cached and rustls
	///   ships it on every handshake.
	/// - `"no_staple"`: the cert has no AIA OCSP URL (or none was
	///   discovered yet) — OCSP isn't applicable here.
	/// - `"fetch_failed"`: the AIA URL is known but the most
	///   recent fetch failed; the scheduler will retry.
	///
	/// Empty for static certs that don't opt into OCSP.
	#[serde(default)]
	pub ocsp_status: String,
	/// RFC 3339 of the cached OCSP staple's `nextUpdate`. `None`
	/// when no staple is cached.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub ocsp_next_update: Option<String>,
	/// AIA-extracted OCSP responder URL. `None` when the cert
	/// doesn't carry one (vane fetches OCSP only when the cert
	/// advertises a responder).
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub ocsp_aia_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AriWindowWire {
	/// RFC 3339 timestamp of the suggested renewal window's start.
	pub start: String,
	/// RFC 3339 timestamp of the suggested renewal window's end.
	pub end: String,
}

#[cfg(test)]
mod tests {
	use super::*;

	fn round_trip<T>(value: &T) -> T
	where
		T: serde::Serialize + for<'de> serde::Deserialize<'de>,
	{
		let s = serde_json::to_string(value).expect("serialize");
		serde_json::from_str(&s).expect("deserialize")
	}

	#[test]
	fn no_args_round_trips() {
		let s = serde_json::to_string(&NoArgs {}).expect("serialize");
		assert_eq!(s, "{}");
		let _back: NoArgs = serde_json::from_str(&s).expect("deserialize");
	}

	#[test]
	fn ping_result_round_trips() {
		let r = PingResult { pong: true, version: env!("CARGO_PKG_VERSION").to_string() };
		assert_eq!(round_trip(&r), r);
	}

	#[test]
	fn stats_result_round_trips() {
		let r = StatsResult {
			uptime_ms: 12_345,
			graph_version_hash: "abcd".to_string(),
			listeners: vec![ListenerStatus {
				addr: "127.0.0.1:8080".to_string(),
				bound: true,
				in_flight_count: 3,
			}],
			flow_log_subscribers: 2,
			tracing_log_subscribers: 1,
		};
		assert_eq!(round_trip(&r), r);
	}

	#[test]
	fn stats_result_decodes_payload_without_subscriber_counts() {
		// Older daemons emit StatsResult without the subscriber counts.
		// `#[serde(default)]` lets the client decode that shape with
		// the counts implicitly zero.
		let raw = r#"{"uptime_ms":1,"graph_version_hash":"00","listeners":[]}"#;
		let r: StatsResult = serde_json::from_str(raw).expect("decode");
		assert_eq!(r.flow_log_subscribers, 0);
		assert_eq!(r.tracing_log_subscribers, 0);
	}

	#[test]
	fn reload_result_swapped_round_trips() {
		let r = ReloadResult::Swapped { hash: "ff".to_string() };
		assert_eq!(round_trip(&r), r);
		// Tagged shape: kind/hash should be flat.
		let value = serde_json::to_value(&r).expect("to_value");
		assert_eq!(value["kind"], "swapped");
		assert_eq!(value["hash"], "ff");
	}

	#[test]
	fn reload_result_unchanged_round_trips() {
		let r = ReloadResult::Unchanged { hash: "00".to_string() };
		assert_eq!(round_trip(&r), r);
		let value = serde_json::to_value(&r).expect("to_value");
		assert_eq!(value["kind"], "unchanged");
	}

	#[test]
	fn shutdown_result_round_trips() {
		let r = ShutdownResult { draining: true };
		assert_eq!(round_trip(&r), r);
	}

	#[test]
	fn get_connections_result_round_trips() {
		let r = GetConnectionsResult {
			listeners: vec![
				ListenerStatus { addr: "127.0.0.1:1".to_string(), bound: true, in_flight_count: 0 },
				ListenerStatus { addr: "127.0.0.1:2".to_string(), bound: false, in_flight_count: 9 },
			],
			connections: vec![ConnectionInfo {
				conn_id: "00000000deadbeef".to_string(),
				listener_addr: "127.0.0.1:1".to_string(),
				remote: "203.0.113.7:54321".to_string(),
				age_ms: 1234,
			}],
		};
		assert_eq!(round_trip(&r), r);
	}

	#[test]
	fn get_connections_result_deserialises_payload_without_connections() {
		// Daemons may emit `{"listeners": [...]}` with no `connections`
		// key. The client must still decode them — `#[serde(default)]`
		// on the field provides that.
		let raw = r#"{"listeners":[{"addr":"127.0.0.1:1","bound":true,"in_flight_count":0}]}"#;
		let r: GetConnectionsResult = serde_json::from_str(raw).expect("decode");
		assert_eq!(r.listeners.len(), 1);
		assert!(r.connections.is_empty());
	}

	#[test]
	fn compile_dry_run_args_round_trips() {
		let a = CompileDryRunArgs { config_dir: "/etc/vaned-b".to_string() };
		let s = serde_json::to_string(&a).expect("serialize");
		let back: CompileDryRunArgs = serde_json::from_str(&s).expect("deserialize");
		assert_eq!(back.config_dir, "/etc/vaned-b");
	}

	#[test]
	fn get_pools_result_round_trips() {
		let r = GetPoolsResult {
			wasm: vec![WasmPoolEntry {
				kind: "stateful".to_string(),
				key: "/etc/vaned/plugins/edge.wasm".to_string(),
				export: "l4-peek".to_string(),
				capacity: 8,
				available: 5,
				in_use: 3,
				total_allocations: 0,
				failures: 0,
			}],
			cgi: Some(CgiPoolEntry {
				cap: 100,
				available: 99,
				in_use: 1,
				total_allocations: 0,
				failures: 0,
			}),
		};
		assert_eq!(round_trip(&r), r);
	}

	#[test]
	fn get_pools_result_decodes_minimal_payload() {
		// Daemons whose `wasm` feature is off should be able to emit
		// `{"cgi": null}` and have clients still decode the result.
		let raw = r#"{"cgi": null}"#;
		let r: GetPoolsResult = serde_json::from_str(raw).expect("decode");
		assert!(r.wasm.is_empty());
		assert!(r.cgi.is_none());
	}

	#[test]
	fn get_upstreams_result_round_trips() {
		let r = GetUpstreamsResult {
			tcp: vec![TcpUpstreamEntry {
				version: "auto".to_string(),
				scheme: "https".to_string(),
				root_ca: "system".to_string(),
				verify_mode: "full".to_string(),
				alpn: vec!["h2".to_string(), "http/1.1".to_string()],
				dns: "system".to_string(),
				fingerprint_id: "abcdef0123456789".to_string(),
			}],
			quic: vec![QuicUpstreamEntry {
				remote_addr: "127.0.0.1:443".to_string(),
				sni: "example.com".to_string(),
				alpn: vec!["h3".to_string()],
				fingerprint_id: "fedcba9876543210".to_string(),
			}],
		};
		assert_eq!(round_trip(&r), r);
	}

	#[test]
	fn get_upstreams_result_decodes_payload_without_quic() {
		// Daemons built without `h3` may emit `{"tcp": [...]}` with no
		// `quic` key. The client must still decode them.
		let raw = r#"{"tcp": []}"#;
		let r: GetUpstreamsResult = serde_json::from_str(raw).expect("decode");
		assert!(r.tcp.is_empty());
		assert!(r.quic.is_empty());
	}
}
