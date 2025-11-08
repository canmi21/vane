/* src/modules/server/l4/health.rs */

use super::model::{Forward, Target};
use crate::modules::ports::model::CONFIG_STATE;
use dashmap::DashMap;
use fancy_log::{LogLevel, log};
use once_cell::sync::Lazy;
use std::{collections::HashSet, time::Duration};
use tokio::{net::TcpStream, task::JoinHandle, time::Instant};

/// Represents the health status of a single target.
#[derive(Debug, Clone)]
pub struct TargetHealth {
	pub available: bool,
	pub latency: Duration,
}

// A global, thread-safe registry for the health status of all targets.
pub static TARGET_HEALTH_REGISTRY: Lazy<DashMap<Target, TargetHealth>> = Lazy::new(DashMap::new);

/// Performs a quick TCP connection test to a target to check its health and latency.
async fn check_target_health(target: Target) {
	let start = Instant::now();
	let timeout = Duration::from_secs(2);

	let check_result = tokio::time::timeout(
		timeout,
		TcpStream::connect((target.ip.as_str(), target.port)),
	)
	.await;

	let health_status = match check_result {
		Ok(Ok(_)) => {
			// Connection succeeded. The stream is dropped immediately.
			TargetHealth {
				available: true,
				latency: start.elapsed(),
			}
		}
		_ => {
			// Connection timed out or failed.
			TargetHealth {
				available: false,
				latency: Duration::MAX,
			}
		}
	};

	TARGET_HEALTH_REGISTRY.insert(target, health_status);
}

/// Gathers all unique targets from the global config and spawns health check tasks for them.
fn run_health_check_cycle() -> Vec<JoinHandle<()>> {
	let mut unique_targets = HashSet::new();
	let config_guard = CONFIG_STATE.load();
	for port_status in config_guard.iter() {
		if let Some(tcp_config) = &port_status.tcp_config {
			for rule in &tcp_config.rules {
				if let super::model::TcpDestination::Forward {
					forward: Forward {
						targets, fallbacks, ..
					},
				} = &rule.destination
				{
					for target in targets.iter().cloned() {
						unique_targets.insert(target);
					}
					for fallback in fallbacks.iter().cloned() {
						unique_targets.insert(fallback);
					}
				}
			}
		}
	}

	// Spawn a task for each target to check its health in parallel and return their handles.
	unique_targets
		.into_iter()
		.map(|target| tokio::spawn(check_target_health(target)))
		.collect()
}

/// Performs an initial health check and waits for all checks to complete.
pub async fn initial_health_check() {
	log(LogLevel::Debug, "⚙ Performing initial health check...");
	let handles = run_health_check_cycle();
	for handle in handles {
		// We wait here to ensure the registry is populated before the app starts serving traffic.
		let _ = handle.await;
	}
	log(LogLevel::Debug, "✓ Initial health check complete.");
}

/// Spawns a background task that periodically checks the health of all configured targets.
pub fn start_periodic_health_checker() {
	log(
		LogLevel::Debug,
		"⚙ Starting periodic L4 target health checker...",
	);
	tokio::spawn(async move {
		let mut interval = tokio::time::interval(Duration::from_secs(5));
		// The first tick is immediate, but we wait for the duration to pass first to avoid
		// running immediately after the initial check.
		loop {
			interval.tick().await;
			// For periodic checks, we "fire and forget", not waiting for them to complete.
			run_health_check_cycle();
		}
	});
}
