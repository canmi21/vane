/* engine/src/modules/ratelimit/gc.rs */

use super::heap::RequestHeap;
use dashmap::DashMap;
use fancy_log::{LogLevel, log};
use std::sync::{Arc, Mutex};
use std::time::Duration;

type RateLimitPool = DashMap<(String, String), Arc<Mutex<RequestHeap>>>;

/// The background task for garbage collection.
/// It periodically checks memory usage and prunes old entries if the limit is exceeded.
pub async fn run_gc_task(pool: Arc<RateLimitPool>, max_memory_bytes: usize) {
	let check_interval = Duration::from_secs(5);
	// We aim to reduce memory to 80% of the limit when GC is triggered.
	let target_memory_bytes = (max_memory_bytes as f64 * 0.8) as usize;

	log(
		LogLevel::Info,
		&format!(
			"Rate limiter GC started. Max memory: {}MB. Check interval: {}s.",
			max_memory_bytes / 1024 / 1024,
			check_interval.as_secs()
		),
	);

	loop {
		tokio::time::sleep(check_interval).await;

		let current_size: usize = pool
			.iter()
			.map(|entry| entry.value().lock().unwrap().memory_size())
			.sum();

		if current_size > max_memory_bytes {
			log(
				LogLevel::Warn,
				&format!(
					"Rate limiter memory usage ({:.2}MB) exceeds limit ({}MB). Starting GC.",
					current_size as f64 / 1024.0 / 1024.0,
					max_memory_bytes / 1024 / 1024
				),
			);

			let bytes_to_free = current_size - target_memory_bytes;
			let bytes_per_record = std::mem::size_of::<std::time::Instant>();

			// Convert bytes to number of records to free.
			let mut records_to_free = (bytes_to_free / bytes_per_record).max(1);

			// Iterate over the pool and prune the oldest records until the target is met.
			for entry in pool.iter() {
				if records_to_free == 0 {
					break;
				}
				entry.value().lock().unwrap().prune_oldest(1);
				records_to_free -= 1;
			}
			log(LogLevel::Info, "Rate limiter GC cycle finished.");
		}
	}
}
