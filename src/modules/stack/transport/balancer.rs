/* src/modules/stack/transport/balancer.rs */

use super::{
	health::{TARGET_HEALTH_REGISTRY, is_udp_target_healthy},
	model::{Forward, ResolvedTarget, Strategy},
	resolver,
};
use dashmap::DashMap;
use once_cell::sync::Lazy;
use rand::prelude::IndexedRandom;
use std::sync::atomic::{AtomicUsize, Ordering};

static SERIAL_COUNTERS: Lazy<DashMap<(u16, String), AtomicUsize>> = Lazy::new(DashMap::new);

/// Selects a TCP target by resolving, checking health, and applying a strategy.
pub async fn select_tcp_target(
	port: u16,
	rule_name: &str,
	forward_config: &Forward,
) -> Option<ResolvedTarget> {
	let resolved_targets = resolver::resolve_targets(&forward_config.targets).await;
	let available_targets: Vec<ResolvedTarget> = resolved_targets
		.into_iter()
		.filter(|t| TARGET_HEALTH_REGISTRY.get(t).map_or(false, |h| h.available))
		.collect();

	let chosen_pool = if !available_targets.is_empty() {
		available_targets
	} else {
		let resolved_fallbacks = resolver::resolve_targets(&forward_config.fallbacks).await;
		resolved_fallbacks
			.into_iter()
			.filter(|t| TARGET_HEALTH_REGISTRY.get(t).map_or(false, |h| h.available))
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
	let available_targets: Vec<ResolvedTarget> = resolved_targets
		.into_iter()
		.filter(|t| is_udp_target_healthy(t))
		.collect();

	let chosen_pool = if !available_targets.is_empty() {
		available_targets
	} else {
		let resolved_fallbacks = resolver::resolve_targets(&forward_config.fallbacks).await;
		resolved_fallbacks
			.into_iter()
			.filter(|t| is_udp_target_healthy(t))
			.collect()
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
			TARGET_HEALTH_REGISTRY
				.get(t)
				.map_or(std::time::Duration::MAX, |h| h.latency)
		}),
		Strategy::Serial => {
			let key = (port, rule_name.to_string());
			let counter = SERIAL_COUNTERS.entry(key).or_default();
			let index = counter.fetch_add(1, Ordering::Relaxed) % pool.len();
			pool.get(index).cloned()
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::modules::stack::transport::health::TargetHealth;
	use serial_test::serial;
	use std::time::Duration;

	/// Helper to create a simple ResolvedTarget for tests.
	fn rt(ip: &str, port: u16) -> ResolvedTarget {
		ResolvedTarget {
			ip: ip.to_string(),
			port,
		}
	}

	/// Cleans up the global registries after a test.
	fn cleanup_globals() {
		TARGET_HEALTH_REGISTRY.clear();
		SERIAL_COUNTERS.clear();
	}

	/// Tests the Random strategy.
	#[test]
	#[serial]
	fn test_choose_from_pool_random() {
		let pool = vec![rt("1.1.1.1", 80), rt("2.2.2.2", 80)];
		let choice = choose_from_pool(80, "test", &Strategy::Random, pool.clone());

		assert!(
			choice.is_some(),
			"Should select a target from a non-empty pool"
		);
		assert!(
			pool.contains(&choice.unwrap()),
			"Selected target must be a member of the pool"
		);
		cleanup_globals();
	}

	/// Tests the Serial (round-robin) strategy.
	#[tokio::test]
	#[serial]
	async fn test_choose_from_pool_serial() {
		let pool = vec![rt("1.1.1.1", 80), rt("2.2.2.2", 80), rt("3.3.3.3", 80)];

		// Call the function 4 times to check for wraparound.
		let choice1 = choose_from_pool(80, "test", &Strategy::Serial, pool.clone());
		let choice2 = choose_from_pool(80, "test", &Strategy::Serial, pool.clone());
		let choice3 = choose_from_pool(80, "test", &Strategy::Serial, pool.clone());
		let choice4 = choose_from_pool(80, "test", &Strategy::Serial, pool.clone());

		assert_eq!(choice1, Some(rt("1.1.1.1", 80)));
		assert_eq!(choice2, Some(rt("2.2.2.2", 80)));
		assert_eq!(choice3, Some(rt("3.3.3.3", 80)));
		assert_eq!(
			choice4,
			Some(rt("1.1.1.1", 80)),
			"Should wrap around to the first target"
		);
		cleanup_globals();
	}

	/// Tests the Fastest strategy based on latency data in the health registry.
	#[test]
	#[serial]
	fn test_choose_from_pool_fastest() {
		let target1 = rt("1.1.1.1", 80);
		let target2_fastest = rt("2.2.2.2", 80);
		let target3 = rt("3.3.3.3", 80);
		let pool = vec![target1.clone(), target2_fastest.clone(), target3.clone()];

		// Populate the health registry with latency info.
		TARGET_HEALTH_REGISTRY.insert(
			target1,
			TargetHealth {
				available: true,
				latency: Duration::from_millis(100),
			},
		);
		TARGET_HEALTH_REGISTRY.insert(
			target2_fastest.clone(),
			TargetHealth {
				available: true,
				latency: Duration::from_millis(10), // The lowest latency
			},
		);
		TARGET_HEALTH_REGISTRY.insert(
			target3,
			TargetHealth {
				available: true,
				latency: Duration::from_millis(200),
			},
		);

		let choice = choose_from_pool(80, "test", &Strategy::Fastest, pool);

		assert_eq!(
			choice,
			Some(target2_fastest),
			"Should select the target with the lowest latency"
		);
		cleanup_globals();
	}

	/// Tests that all strategies return None when the pool is empty.
	#[test]
	#[serial]
	fn test_choose_from_pool_empty() {
		let pool: Vec<ResolvedTarget> = vec![];
		let random_choice = choose_from_pool(80, "test", &Strategy::Random, pool.clone());
		let serial_choice = choose_from_pool(80, "test", &Strategy::Serial, pool.clone());
		let fastest_choice = choose_from_pool(80, "test", &Strategy::Fastest, pool.clone());

		assert!(
			random_choice.is_none(),
			"Random should return None for empty pool"
		);
		assert!(
			serial_choice.is_none(),
			"Serial should return None for empty pool"
		);
		assert!(
			fastest_choice.is_none(),
			"Fastest should return None for empty pool"
		);
		cleanup_globals();
	}
}
