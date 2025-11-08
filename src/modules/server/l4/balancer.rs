/* src/modules/server/l4/balancer.rs */

use super::{
	health::TARGET_HEALTH_REGISTRY,
	model::{Forward, Strategy, Target},
};
use dashmap::DashMap;
use once_cell::sync::Lazy;
use rand::prelude::IndexedRandom;
use std::sync::atomic::{AtomicUsize, Ordering};

// Global state for Serial (Round Robin) counters.
// The key is a unique identifier for the rule: (listening_port, rule_name).
static SERIAL_COUNTERS: Lazy<DashMap<(u16, String), AtomicUsize>> = Lazy::new(DashMap::new);

/// Selects a target from a forward configuration based on health and strategy.
pub fn select_target(port: u16, rule_name: &str, forward_config: &Forward) -> Option<Target> {
	// Filter primary targets that are available.
	let available_targets: Vec<&Target> = forward_config
		.targets
		.iter()
		.filter(|t| {
			TARGET_HEALTH_REGISTRY
				.get(*t)
				.map_or(false, |h| h.available)
		})
		.collect();

	// If there are available primary targets, use them. Otherwise, try fallbacks.
	let chosen_pool = if !available_targets.is_empty() {
		available_targets
	} else {
		forward_config
			.fallbacks
			.iter()
			.filter(|t| {
				TARGET_HEALTH_REGISTRY
					.get(*t)
					.map_or(false, |h| h.available)
			})
			.collect()
	};

	if chosen_pool.is_empty() {
		return None; // No available targets in either pool.
	}

	match forward_config.strategy {
		Strategy::Random => {
			// Create a thread-local random number generator.
			let mut rng = rand::rng();
			// .choose() is provided by the SliceRandom trait.
			chosen_pool.choose(&mut rng).map(|t| (*t).clone())
		}
		Strategy::Fastest => chosen_pool
			.iter()
			.min_by_key(|t| {
				TARGET_HEALTH_REGISTRY
					.get(*t)
					.map_or(std::time::Duration::MAX, |h| h.latency)
			})
			.map(|t| (*t).clone()),
		Strategy::Serial => {
			let key = (port, rule_name.to_string());
			let counter = SERIAL_COUNTERS.entry(key).or_default();
			// fetch_add provides atomic increment and returns the old value.
			let index = counter.fetch_add(1, Ordering::Relaxed) % chosen_pool.len();
			chosen_pool.get(index).map(|t| (*t).clone())
		}
	}
}
