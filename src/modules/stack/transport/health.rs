/* src/modules/stack/transport/health.rs */

use super::{legacy::tcp::TcpDestination, model::ResolvedTarget, resolver, tcp::TcpConfig};
use crate::{common::getenv, modules::ports::model::CONFIG_STATE};
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

impl TargetHealth {
	fn unhealthy() -> Self {
		TargetHealth {
			available: false,
			latency: Duration::MAX,
		}
	}
}

pub static TARGET_HEALTH_REGISTRY: Lazy<DashMap<ResolvedTarget, TargetHealth>> =
	Lazy::new(DashMap::new);
static UNHEALTHY_UDP_TARGETS: Lazy<DashMap<ResolvedTarget, Instant>> = Lazy::new(DashMap::new);

async fn check_tcp_target_health(target: ResolvedTarget, timeout_ms: u64) {
	let start = Instant::now();
	let timeout = Duration::from_millis(timeout_ms);
	let check_result = tokio::time::timeout(
		timeout,
		TcpStream::connect((target.ip.as_str(), target.port)),
	)
	.await;

	let health_status = match check_result {
		Ok(Ok(_)) => {
			let latency = start.elapsed();
			if let Some(existing) = TARGET_HEALTH_REGISTRY.get_mut(&target) {
				if !existing.available {
					log(
						LogLevel::Info,
						&format!(
							"✓ TCP target {}:{} has recovered and is back online (latency: {:?}).",
							target.ip, target.port, latency
						),
					);
				}
			}
			TargetHealth {
				available: true,
				latency,
			}
		}
		_ => TargetHealth::unhealthy(),
	};
	TARGET_HEALTH_REGISTRY.insert(target, health_status);
}

async fn run_health_check_cycle() -> Vec<JoinHandle<()>> {
	let mut unique_targets = HashSet::new();
	let config_guard = CONFIG_STATE.load();

	let connect_timeout_ms = getenv::get_env("HEALTH_TCP_CONNECT_TIMEOUT_MS", "2000".to_string())
		.parse::<u64>()
		.unwrap_or(2000);

	for port_status in config_guard.iter() {
		if let Some(tcp_config) = &port_status.tcp_config {
			if let TcpConfig::Legacy(legacy_config) = &**tcp_config {
				for rule in &legacy_config.rules {
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
	}
	unique_targets
		.into_iter()
		.map(|target| tokio::spawn(check_tcp_target_health(target, connect_timeout_ms)))
		.collect()
}

pub fn mark_tcp_target_unhealthy(target: &ResolvedTarget) {
	if TARGET_HEALTH_REGISTRY
		.get(target)
		.map_or(true, |h| h.available)
	{
		log(
			LogLevel::Warn,
			&format!(
				"✗ Proactively marked TCP target {}:{} as unavailable due to connection failure.",
				target.ip, target.port
			),
		);
		TARGET_HEALTH_REGISTRY.insert(target.clone(), TargetHealth::unhealthy());
	}
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
		let interval_secs = getenv::get_env("HEALTH_TCP_INTERVAL_SECS", "5".to_string())
			.parse::<u64>()
			.unwrap_or(5);
		let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
		loop {
			interval.tick().await;
			let handles = run_health_check_cycle().await;
			for handle in handles {
				let _ = handle.await;
			}
		}
	});
	tokio::spawn(async move {
		let interval_secs = getenv::get_env("HEALTH_UDP_CLEANUP_INTERVAL_SECS", "5".to_string())
			.parse::<u64>()
			.unwrap_or(5);
		let unhealthy_ttl_secs = getenv::get_env("HEALTH_UDP_UNHEALTHY_TTL_SECS", "10".to_string())
			.parse::<u64>()
			.unwrap_or(10);

		let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
		let unhealthy_ttl = Duration::from_secs(unhealthy_ttl_secs);
		loop {
			interval.tick().await;
			UNHEALTHY_UDP_TARGETS.retain(|_, v| v.elapsed() < unhealthy_ttl);
		}
	});
}
