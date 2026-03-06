/* src/engine/src/shared/balancer.rs */

use crate::shared::{
	health::{TARGET_HEALTH_REGISTRY, is_udp_target_healthy},
	resolver,
};
use dashmap::DashMap;
use rand::prelude::IndexedRandom;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use vane_primitives::model::{Forward, ResolvedTarget, Strategy};

static SERIAL_COUNTERS: LazyLock<DashMap<(u16, String), AtomicUsize>> = LazyLock::new(DashMap::new);

/// Selects a TCP target by resolving, checking health, and applying a strategy.
pub async fn select_tcp_target(
	port: u16,
	rule_name: &str,
	forward_config: &Forward,
) -> Option<ResolvedTarget> {
	let resolved_targets = resolver::resolve_targets(&forward_config.targets).await;
	let available_targets: Vec<ResolvedTarget> = resolved_targets
		.into_iter()
		.filter(|t| TARGET_HEALTH_REGISTRY.get(t).is_some_and(|h| h.available))
		.collect();

	let chosen_pool = if !available_targets.is_empty() {
		available_targets
	} else {
		let resolved_fallbacks = resolver::resolve_targets(&forward_config.fallbacks).await;
		resolved_fallbacks
			.into_iter()
			.filter(|t| TARGET_HEALTH_REGISTRY.get(t).is_some_and(|h| h.available))
			.collect()
	};
	choose_from_pool(port, rule_name, &forward_config.strategy, chosen_pool)
}

/// Selects a UDP target by resolving, checking health, and applying a strategy.
pub async fn select_udp_target(
	port: u16,
	rule_name: &str,
	forward_config: &Forward,
) -> Option<ResolvedTarget> {
	let resolved_targets = resolver::resolve_targets(&forward_config.targets).await;
	let available_targets: Vec<ResolvedTarget> =
		resolved_targets.into_iter().filter(is_udp_target_healthy).collect();

	let chosen_pool = if !available_targets.is_empty() {
		available_targets
	} else {
		let resolved_fallbacks = resolver::resolve_targets(&forward_config.fallbacks).await;
		resolved_fallbacks.into_iter().filter(is_udp_target_healthy).collect()
	};
	choose_from_pool(port, rule_name, &forward_config.strategy, chosen_pool)
}

/// Chooses a target from a pool of resolved targets based on the configured strategy.
fn choose_from_pool(
	port: u16,
	rule_name: &str,
	strategy: &Strategy,
	pool: Vec<ResolvedTarget>,
) -> Option<ResolvedTarget> {
	if pool.is_empty() {
		return None;
	}
	match strategy {
		Strategy::Random => {
			let mut rng = rand::rng();
			pool.choose(&mut rng).cloned()
		}
		Strategy::Fastest => pool.into_iter().min_by_key(|t| {
			TARGET_HEALTH_REGISTRY.get(t).map_or(std::time::Duration::MAX, |h| h.latency)
		}),
		Strategy::Serial => {
			let key = (port, rule_name.to_owned());
			let counter = SERIAL_COUNTERS.entry(key).or_default();
			let index = counter.fetch_add(1, Ordering::Relaxed) % pool.len();
			pool.get(index).cloned()
		}
	}
}
