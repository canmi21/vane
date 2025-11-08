/* src/modules/server/l4/health.rs */

use super::model::{Forward, Target};
use crate::modules::ports::model::CONFIG_STATE;
use dashmap::DashMap;
use fancy_log::{LogLevel, log};
use once_cell::sync::Lazy;
use std::{collections::HashSet, time::Duration};
use tokio::{net::TcpStream, time::Instant};

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

/// A background task that periodically checks the health of all configured targets.
pub fn start_health_checker_task() {
	log(LogLevel::Debug, "⚙ Starting L4 target health checker...");
	tokio::spawn(async move {
		let mut interval = tokio::time::interval(Duration::from_secs(5));
		loop {
			interval.tick().await;

			// Collect all unique targets from the current global config.
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
				// NOTE: UDP health checks could be added here in the future.
			}

			// Spawn a task for each target to check its health in parallel.
			for target in unique_targets {
				tokio::spawn(check_target_health(target));
			}
		}
	});
}
