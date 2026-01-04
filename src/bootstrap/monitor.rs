/* src/bootstrap/monitor.rs */

use crate::common::{config::getenv, sys::system};
use crate::layers::l7::container;
use fancy_log::{LogLevel, log};
use tokio::time::{Duration, sleep};

/// Starts the background L7 memory monitor.
pub async fn start_l7_memory_monitor() {
	let adaptive_enabled =
		getenv::get_env("L7_ADAPTIVE_MEMORY_LIMIT", "true".to_string()).to_lowercase() == "true";
	let ratio = getenv::get_env("L7_ADAPTIVE_MEMORY_RATIO", "85".to_string())
		.parse::<u64>()
		.unwrap_or(85)
		.min(95);
	let fallback_limit = getenv::get_env("L7_GLOBAL_BUFFER_LIMIT", "536870912".to_string())
		.parse::<usize>()
		.unwrap_or(536_870_912);

	if !adaptive_enabled {
		log(
			LogLevel::Info,
			&format!("⚙ L7 Memory Limit: Fixed ({} bytes)", fallback_limit),
		);
		container::update_memory_limit(fallback_limit);
		return;
	}

	if !system::is_adaptive_supported() {
		log(
			LogLevel::Warn,
			"⚠ Adaptive memory management not supported on this platform. Falling back to fixed limit.",
		);
		container::update_memory_limit(fallback_limit);
		return;
	}

	log(
		LogLevel::Info,
		&format!(
			"✓ L7 Memory Limit: Adaptive (Ratio: {}%, Fallback: {} bytes)",
			ratio, fallback_limit
		),
	);

	tokio::spawn(async move {
		loop {
			if let Some(free_mem) = system::get_free_memory() {
				let used_by_vane =
					container::GLOBAL_L7_BUFFERED_BYTES.load(std::sync::atomic::Ordering::Relaxed) as u64;
				let calculated_limit = (free_mem * ratio / 100) + used_by_vane;

				// Apply limit
				container::update_memory_limit(calculated_limit as usize);
			} else {
				// Unexpected failure during runtime detection
				container::update_memory_limit(fallback_limit);
			}
			sleep(Duration::from_secs(1)).await;
		}
	});
}
