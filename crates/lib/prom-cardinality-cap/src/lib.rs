//! Track unique `(metric_name, label_set)` combinations per "tenant"
//! namespace and silently drop new ones once a cap is reached,
//! emitting a `tracing::warn!` exactly once on the first drop per
//! namespace.
//!
//! See the README for the kind of system that needs this guardrail.

#![deny(unsafe_code)]
#![warn(unreachable_pub)]

use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;
use std::sync::Mutex;

#[derive(Debug)]
struct NamespaceCardinality {
	seen: HashSet<u64>,
	warned_at_cap: bool,
}

impl NamespaceCardinality {
	fn new() -> Self {
		Self { seen: HashSet::new(), warned_at_cap: false }
	}
}

/// A bounded cardinality tracker, sharded by `Arc<str>` namespace.
/// Each namespace gets its own slate of seen `(name, labels)` tuples
/// and its own warn-once flag.
#[derive(Debug)]
pub struct CardinalityRegistry {
	cap: usize,
	state: Mutex<HashMap<Arc<str>, NamespaceCardinality>>,
}

impl CardinalityRegistry {
	/// Construct a registry with an explicit cap. The cap applies
	/// independently to each namespace passed to [`try_admit`].
	#[must_use]
	pub fn with_cap(cap: usize) -> Self {
		Self { cap, state: Mutex::new(HashMap::new()) }
	}

	/// Returns `true` when the `(metric_name, labels)` tuple is
	/// admitted (already-seen or under-cap), `false` when blocked
	/// by the cap. The first block on a given namespace emits a
	/// single `tracing::warn!`; subsequent blocks on the same
	/// namespace stay silent so a hot loop in caller code does not
	/// flood the structured log.
	///
	/// `labels` is hashed in a sort-stable order — callers do not
	/// have to sort beforehand.
	///
	/// # Panics
	/// Panics if the inner `Mutex` is poisoned.
	pub fn try_admit(
		&self,
		namespace: &Arc<str>,
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
		let entry = state.entry(Arc::clone(namespace)).or_insert_with(NamespaceCardinality::new);
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
				namespace = %namespace,
				cap = self.cap,
				"metric cardinality cap reached; subsequent unique series dropped",
			);
		}
		false
	}

	/// Number of distinct series tracked for `namespace`.
	///
	/// # Panics
	/// Panics if the inner `Mutex` is poisoned.
	#[must_use]
	pub fn series_count(&self, namespace: &Arc<str>) -> usize {
		self.state.lock().unwrap().get(namespace).map_or(0, |e| e.seen.len())
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn ns(name: &str) -> Arc<str> {
		Arc::from(name)
	}

	#[test]
	fn admits_distinct_tuples_under_cap() {
		let r = CardinalityRegistry::with_cap(3);
		let m = ns("/path/a.wasm");
		assert!(r.try_admit(&m, "x", &[]));
		assert!(r.try_admit(&m, "y", &[]));
		assert!(r.try_admit(&m, "z", &[]));
		assert_eq!(r.series_count(&m), 3);
	}

	#[test]
	fn second_emit_of_same_tuple_is_admitted_without_growing() {
		let r = CardinalityRegistry::with_cap(3);
		let m = ns("/path/a.wasm");
		assert!(r.try_admit(&m, "x", &[("k".into(), "1".into())]));
		assert!(r.try_admit(&m, "x", &[("k".into(), "1".into())]));
		assert_eq!(r.series_count(&m), 1);
	}

	#[test]
	fn label_order_does_not_create_distinct_series() {
		let r = CardinalityRegistry::with_cap(3);
		let m = ns("/path/a.wasm");
		assert!(r.try_admit(&m, "x", &[("a".into(), "1".into()), ("b".into(), "2".into())]));
		assert!(r.try_admit(&m, "x", &[("b".into(), "2".into()), ("a".into(), "1".into())]));
		assert_eq!(r.series_count(&m), 1, "sort-stable hashing collapses label-order variants");
	}

	#[test]
	fn over_cap_emissions_are_dropped_silently_after_first_warn() {
		let r = CardinalityRegistry::with_cap(2);
		let m = ns("/path/a.wasm");
		assert!(r.try_admit(&m, "a", &[]));
		assert!(r.try_admit(&m, "b", &[]));
		// Three further unique tuples — all must be dropped, only the
		// first crossing emits a tracing event (the test cannot directly
		// observe `tracing::warn!` without a capture layer, but the
		// boolean return contract is the public surface).
		for n in 0..3 {
			let name = format!("x{n}");
			assert!(!r.try_admit(&m, &name, &[]), "over-cap emit must be rejected");
		}
		assert_eq!(r.series_count(&m), 2, "cap holds firm");
	}

	#[test]
	fn namespaces_have_independent_caps() {
		let r = CardinalityRegistry::with_cap(2);
		let a = ns("/path/a.wasm");
		let b = ns("/path/b.wasm");
		assert!(r.try_admit(&a, "x", &[]));
		assert!(r.try_admit(&a, "y", &[]));
		assert!(!r.try_admit(&a, "z", &[]));
		// Namespace B has its own slate.
		assert!(r.try_admit(&b, "x", &[]));
		assert!(r.try_admit(&b, "y", &[]));
		assert_eq!(r.series_count(&a), 2);
		assert_eq!(r.series_count(&b), 2);
	}
}
