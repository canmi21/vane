//! Per-`WasmtimeRuntime` metric cardinality cap.
//!
//! Plugins emit `host.metric-counter` / `host.metric-gauge` with
//! arbitrary `(name, labels)` tuples; without bounds, a misbehaving
//! plugin can drive the metrics registry into millions of unique
//! series and blow daemon RAM. The registry tracks the unique tuples
//! per module and silently drops emissions past the cap.
//!
//! Cap is resolved once at runtime construction from
//! `VANE_WASM_METRIC_CARDINALITY_CAP` (default 1000) and is shared
//! across every export of every loaded module via an `Arc`. State
//! lives on the runtime, not in process-static — flow-graph reload
//! drops the old runtime, which drops the registry, which resets
//! the per-module cardinality counts.
//!
//! When the cap fires for the first time on a module, a single
//! `WARN` is emitted. Subsequent overshoots stay silent so a hot
//! loop in plugin code does not flood the structured log.

use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;
use std::sync::Mutex;

const DEFAULT_CAP: usize = 1000;

#[derive(Debug)]
struct ModuleCardinality {
	seen: HashSet<u64>,
	warned_at_cap: bool,
}

impl ModuleCardinality {
	fn new() -> Self {
		Self { seen: HashSet::new(), warned_at_cap: false }
	}
}

#[derive(Debug)]
pub struct CardinalityRegistry {
	cap: usize,
	state: Mutex<HashMap<Arc<str>, ModuleCardinality>>,
}

impl CardinalityRegistry {
	/// Construct a registry whose cap is read from
	/// `VANE_WASM_METRIC_CARDINALITY_CAP` (default 1000). A cap of
	/// zero is rejected — the parsed value stays as-is when valid,
	/// otherwise we fall through to the default.
	#[must_use]
	pub fn from_env() -> Self {
		let cap = std::env::var("VANE_WASM_METRIC_CARDINALITY_CAP")
			.ok()
			.and_then(|s| s.parse::<usize>().ok())
			.filter(|n| *n > 0)
			.unwrap_or(DEFAULT_CAP);
		Self::with_cap(cap)
	}

	/// Construct a registry with an explicit cap. Test-only; the
	/// production constructor is `from_env`.
	#[must_use]
	pub fn with_cap(cap: usize) -> Self {
		Self { cap, state: Mutex::new(HashMap::new()) }
	}

	/// Returns `true` when the `(metric_name, labels)` tuple is
	/// admitted (already-seen or under-cap), `false` when blocked
	/// by the cap. The first block on a module emits a single
	/// `WARN`; subsequent blocks on the same module stay silent.
	///
	/// `labels` is hashed in a sort-stable order — callers do not
	/// have to sort beforehand.
	///
	/// # Panics
	/// Panics if the inner `Mutex` is poisoned.
	pub fn try_admit(
		&self,
		module_id: &Arc<str>,
		metric_name: &str,
		labels: &[(String, String)],
	) -> bool {
		let mut sorted: Vec<&(String, String)> = labels.iter().collect();
		sorted.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
		let mut hasher = DefaultHasher::new();
		metric_name.hash(&mut hasher);
		for (k, v) in sorted {
			k.hash(&mut hasher);
			v.hash(&mut hasher);
		}
		let key = hasher.finish();

		let mut state = self.state.lock().unwrap();
		let entry = state.entry(Arc::clone(module_id)).or_insert_with(ModuleCardinality::new);
		if entry.seen.contains(&key) {
			return true;
		}
		if entry.seen.len() < self.cap {
			entry.seen.insert(key);
			return true;
		}
		if !entry.warned_at_cap {
			entry.warned_at_cap = true;
			tracing::warn!(
				target: "vane::wasm",
				module_id = %module_id,
				cap = self.cap,
				"metric cardinality cap reached; subsequent unique series dropped",
			);
		}
		false
	}

	/// Number of distinct series tracked for `module_id`. Test
	/// helper — production code never inspects the registry.
	#[doc(hidden)]
	#[must_use]
	pub fn series_count_for_test(&self, module_id: &Arc<str>) -> usize {
		self.state.lock().unwrap().get(module_id).map_or(0, |e| e.seen.len())
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn module(name: &str) -> Arc<str> {
		Arc::from(name)
	}

	#[test]
	fn admits_distinct_tuples_under_cap() {
		let r = CardinalityRegistry::with_cap(3);
		let m = module("/path/a.wasm");
		assert!(r.try_admit(&m, "x", &[]));
		assert!(r.try_admit(&m, "y", &[]));
		assert!(r.try_admit(&m, "z", &[]));
		assert_eq!(r.series_count_for_test(&m), 3);
	}

	#[test]
	fn second_emit_of_same_tuple_is_admitted_without_growing() {
		let r = CardinalityRegistry::with_cap(3);
		let m = module("/path/a.wasm");
		assert!(r.try_admit(&m, "x", &[("k".into(), "1".into())]));
		assert!(r.try_admit(&m, "x", &[("k".into(), "1".into())]));
		assert_eq!(r.series_count_for_test(&m), 1);
	}

	#[test]
	fn label_order_does_not_create_distinct_series() {
		let r = CardinalityRegistry::with_cap(3);
		let m = module("/path/a.wasm");
		assert!(r.try_admit(&m, "x", &[("a".into(), "1".into()), ("b".into(), "2".into())],));
		assert!(r.try_admit(&m, "x", &[("b".into(), "2".into()), ("a".into(), "1".into())],));
		assert_eq!(
			r.series_count_for_test(&m),
			1,
			"sort-stable hashing collapses label-order variants"
		);
	}

	#[test]
	fn over_cap_emissions_are_dropped_silently_after_first_warn() {
		let r = CardinalityRegistry::with_cap(2);
		let m = module("/path/a.wasm");
		assert!(r.try_admit(&m, "a", &[]));
		assert!(r.try_admit(&m, "b", &[]));
		// Three further unique tuples — all must be dropped, only
		// the first crossing emits a tracing event (the test cannot
		// directly observe `tracing::warn!` without a capture layer,
		// but the boolean return contract is the public surface).
		for n in 0..3 {
			let name = format!("x{n}");
			assert!(!r.try_admit(&m, &name, &[]), "over-cap emit must be rejected");
		}
		assert_eq!(r.series_count_for_test(&m), 2, "cap holds firm");
	}

	#[test]
	fn modules_have_independent_caps() {
		let r = CardinalityRegistry::with_cap(2);
		let a = module("/path/a.wasm");
		let b = module("/path/b.wasm");
		assert!(r.try_admit(&a, "x", &[]));
		assert!(r.try_admit(&a, "y", &[]));
		assert!(!r.try_admit(&a, "z", &[]));
		// Module B has its own slate.
		assert!(r.try_admit(&b, "x", &[]));
		assert!(r.try_admit(&b, "y", &[]));
		assert_eq!(r.series_count_for_test(&a), 2);
		assert_eq!(r.series_count_for_test(&b), 2);
	}
}
