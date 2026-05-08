//! Per-`WasmtimeRuntime` metric cardinality cap.
//!
//! Plugins emit `host.metric-counter` / `host.metric-gauge` with
//! arbitrary `(name, labels)` tuples; without bounds, a misbehaving
//! plugin can drive the metrics registry into millions of unique
//! series and blow daemon RAM. The registry tracks the unique tuples
//! per module and silently drops emissions past the cap.
//!
//! The cap is resolved at runtime construction from
//! `VANE_WASM_METRIC_CARDINALITY_CAP` (default 1000) and is shared
//! across every export of every loaded module via an `Arc`. State
//! lives on the runtime, not in process-static — flow-graph reload
//! drops the old runtime, which drops the registry, which resets
//! the per-module cardinality counts.
//!
//! The implementation lives in [`prom_cardinality_cap`]; this module
//! re-exports it under the name vane callers know and provides the
//! vane-specific `from_env` constructor.

pub use prom_cardinality_cap::CardinalityRegistry;

const DEFAULT_CAP: usize = 1000;

/// Construct a [`CardinalityRegistry`] whose cap is read from
/// `VANE_WASM_METRIC_CARDINALITY_CAP` (default 1000). A cap of zero
/// is rejected — the parsed value stays as-is when valid, otherwise
/// we fall through to the default.
#[must_use]
pub(crate) fn registry_from_env() -> CardinalityRegistry {
	let cap = std::env::var("VANE_WASM_METRIC_CARDINALITY_CAP")
		.ok()
		.and_then(|s| s.parse::<usize>().ok())
		.filter(|n| *n > 0)
		.unwrap_or(DEFAULT_CAP);
	CardinalityRegistry::with_cap(cap)
}
