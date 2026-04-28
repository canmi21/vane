//! Per-verb argument and result schemas. Wire-side `Request.args` is a
//! `serde_json::Value` (untyped on the wire); each verb's handler
//! deserialises it into its own typed struct, surfacing
//! [`crate::protocol::WireErrorKind::BadArgs`] on shape mismatch.
//!
//! Verbs: `compile_dry_run`, `reload`, `get_config`, `stats`,
//! `shutdown`, `get_connections`, plus `ping` for cheap liveness checks.
//!
//! See `spec/architecture/10-management.md` § _Verbs_.

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
		};
		assert_eq!(round_trip(&r), r);
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
}
