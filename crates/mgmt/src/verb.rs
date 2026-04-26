//! Per-verb argument and result schemas. Wire-side `Request.args` is a
//! `serde_json::Value` (untyped on the wire); each verb's handler
//! deserialises it into its own typed struct, surfacing
//! [`crate::protocol::WireErrorKind::BadArgs`] on shape mismatch.
//!
//! Stage 1 verbs: `compile_dry_run`, `reload`, `get_active_config`,
//! `stats`, `shutdown`, `list_connections`, plus `ping` for cheap
//! liveness checks.
//!
//! See `spec/architecture/10-management.md` § _Verbs_.

use serde::{Deserialize, Serialize};

// ─── Verb names ─────────────────────────────────────────────────────────
pub const VERB_PING: &str = "ping";
pub const VERB_STATS: &str = "stats";
pub const VERB_SHUTDOWN: &str = "shutdown";
pub const VERB_GET_ACTIVE_CONFIG: &str = "get_active_config";
pub const VERB_RELOAD: &str = "reload";
pub const VERB_COMPILE_DRY_RUN: &str = "compile_dry_run";
pub const VERB_LIST_CONNECTIONS: &str = "list_connections";

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

// ─── get_active_config ──────────────────────────────────────────────────
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetActiveConfigResult {
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

// ─── list_connections ──────────────────────────────────────────────────
/// Per-listener summary. Per-connection details require the listener
/// set to register `ConnContext`s in a registry — deferred to a later
/// chunk. For now this returns the same shape as `StatsResult.listeners`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ListConnectionsResult {
	pub listeners: Vec<ListenerStatus>,
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
		let r = PingResult { pong: true, version: "0.10.0".to_string() };
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
	fn list_connections_result_round_trips() {
		let r = ListConnectionsResult {
			listeners: vec![
				ListenerStatus { addr: "127.0.0.1:1".to_string(), bound: true, in_flight_count: 0 },
				ListenerStatus { addr: "127.0.0.1:2".to_string(), bound: false, in_flight_count: 9 },
			],
		};
		assert_eq!(round_trip(&r), r);
	}

	#[test]
	fn compile_dry_run_args_round_trips() {
		let a = CompileDryRunArgs { config_dir: "/etc/vaned-b".to_string() };
		let s = serde_json::to_string(&a).expect("serialize");
		let back: CompileDryRunArgs = serde_json::from_str(&s).expect("deserialize");
		assert_eq!(back.config_dir, "/etc/vaned-b");
	}
}
