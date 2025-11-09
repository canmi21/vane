/* src/modules/stack/transport/health.rs */

use super::{
	model::{ResolvedTarget, TcpDestination},
	resolver,
};
use crate::modules::ports::model::CONFIG_STATE;
use dashmap::DashMap;
use fancy_log::{LogLevel, log};
use once_cell::sync::Lazy;
use std::{collections::HashSet, time::Duration};
use tokio::{net::TcpStream, task::JoinHandle, time::Instant};

#[derive(Debug, Clone)]
pub struct TargetHealth {
	pub available: bool,
	pub latency: Duration,
}

pub static TARGET_HEALTH_REGISTRY: Lazy<DashMap<ResolvedTarget, TargetHealth>> =
	Lazy::new(DashMap::new);
static UNHEALTHY_UDP_TARGETS: Lazy<DashMap<ResolvedTarget, Instant>> = Lazy::new(DashMap::new);

/// Performs a quick TCP connection test to a resolved target.
async fn check_tcp_target_health(target: ResolvedTarget) {
	let start = Instant::now();
	let timeout = Duration::from_secs(2);
	let check_result = tokio::time::timeout(
		timeout,
		TcpStream::connect((target.ip.as_str(), target.port)),
	)
	.await;
	let health_status = match check_result {
		Ok(Ok(_)) => TargetHealth {
			available: true,
			latency: start.elapsed(),
		},
		_ => TargetHealth {
			available: false,
			latency: Duration::MAX,
		},
	};
	TARGET_HEALTH_REGISTRY.insert(target, health_status);
}

/// Gathers all unique targets, resolves them, and spawns health checks.
async fn run_health_check_cycle() -> Vec<JoinHandle<()>> {
	let mut unique_targets = HashSet::new();
	let config_guard = CONFIG_STATE.load();

	for port_status in config_guard.iter() {
		if let Some(tcp_config) = &port_status.tcp_config {
			for rule in &tcp_config.rules {
				if let TcpDestination::Forward { forward } = &rule.destination {
					for rt in resolver::resolve_targets(&forward.targets).await {
						unique_targets.insert(rt);
					}
					for rt in resolver::resolve_targets(&forward.fallbacks).await {
						unique_targets.insert(rt);
					}
				}
			}
		}
	}
	unique_targets
		.into_iter()
		.map(|target| tokio::spawn(check_tcp_target_health(target)))
		.collect()
}

pub fn mark_udp_target_unhealthy(target: &ResolvedTarget) {
	UNHEALTHY_UDP_TARGETS.insert(target.clone(), Instant::now());
}

pub fn is_udp_target_healthy(target: &ResolvedTarget) -> bool {
	!UNHEALTHY_UDP_TARGETS.contains_key(target)
}

pub async fn initial_health_check() {
	log(
		LogLevel::Debug,
		"⚙ Performing initial health check for TCP targets...",
	);
	let handles = run_health_check_cycle().await;
	for handle in handles {
		let _ = handle.await;
	}
	log(LogLevel::Debug, "✓ Initial TCP health check complete.");
}

pub fn start_periodic_health_checkers() {
	log(LogLevel::Debug, "⚙ Starting periodic health checkers...");
	tokio::spawn(async move {
		let mut interval = tokio::time::interval(Duration::from_secs(5));
		loop {
			interval.tick().await;
			let _ = run_health_check_cycle().await;
		}
	});
	tokio::spawn(async move {
		let mut interval = tokio::time::interval(Duration::from_secs(5));
		let unhealthy_ttl = Duration::from_secs(10);
		loop {
			interval.tick().await;
			UNHEALTHY_UDP_TARGETS.retain(|_, v| v.elapsed() < unhealthy_ttl);
		}
	});
}
