/* src/core/src/bootstrap/monitor.rs */

use fancy_log::{LogLevel, log};
use tokio::time::{Duration, sleep};
use vane_app::l7::container;
use vane_primitives::common::sys::system;

/// Starts the background L7 memory monitor.
pub async fn start_l7_memory_monitor() {
	let adaptive_enabled = envflag::get::<bool>("L7_ADAPTIVE_MEMORY_LIMIT", true);
	let ratio = envflag::get::<u64>("L7_ADAPTIVE_MEMORY_RATIO", 85).min(95);
	let fallback_limit = envflag::get::<usize>("L7_GLOBAL_BUFFER_LIMIT", 536_870_912);

	if !adaptive_enabled {
		log(LogLevel::Info, &format!("⚙ L7 Memory Limit: Fixed ({fallback_limit} bytes)"));
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
		&format!("✓ L7 Memory Limit: Adaptive (Ratio: {ratio}%, Fallback: {fallback_limit} bytes)"),
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
