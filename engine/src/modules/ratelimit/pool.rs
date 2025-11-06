/* engine/src/modules/ratelimit/pool.rs */

use super::{gc, heap::RequestHeap};
use dashmap::DashMap;
use once_cell::sync::Lazy;
use std::env;
use std::sync::{Arc, Mutex};
use std::time::Instant;

// The global, thread-safe pool for all rate limiters.
// Key: (Identifier String, Context String)
// Value: A heap of request timestamps, protected by a Mutex for interior mutability.
type RateLimitPool = DashMap<(String, String), Arc<Mutex<RequestHeap>>>;
static POOL: Lazy<Arc<RateLimitPool>> = Lazy::new(|| Arc::new(DashMap::new()));

/// Initializes the rate limiter system and starts its garbage collection task.
/// This should be called once on application startup.
pub fn start_gc_task() {
	let max_memory_mb = env::var("RATE_LIMITER_MAX_MEMORY_MB")
		.ok()
		.and_then(|s| s.parse::<usize>().ok())
		.unwrap_or(16); // Default to 16MB
	let max_memory_bytes = max_memory_mb * 1024 * 1024;

	let pool_clone = POOL.clone();
	tokio::spawn(async move {
		gc::run_gc_task(pool_clone, max_memory_bytes).await;
	});
}

/// Checks if a request is allowed based on its ID, context, and a given limit.
///
/// # Arguments
/// * `id`: A unique identifier for the entity being limited (e.g., IP address, user ID).
/// * `context`: A namespace for the limit (e.g., domain name, API path).
/// * `limit`: The number of allowed requests per second.
///
/// # Returns
/// * `true` if the request is within the limit, `false` otherwise.
pub fn check(id: &str, context: &str, limit: u32) -> bool {
	// A limit of 0 means no requests are allowed.
	if limit == 0 {
		return false;
	}

	let key = (id.to_string(), context.to_string());

	// Use `entry` for efficient, atomic access.
	let heap_arc = POOL
		.entry(key)
		.or_insert_with(|| Arc::new(Mutex::new(RequestHeap::new())))
		.clone();

	// Lock the mutex for this specific heap to modify it.
	let mut heap = heap_arc.lock().unwrap();

	heap.add_and_check(Instant::now(), limit)
}

// --- Tests ---
#[cfg(test)]
mod tests {
	use super::*;
	use serial_test::serial;
	use std::time::Duration;

	#[tokio::test]
	#[serial]
	async fn test_rate_limiter_basic_functionality() {
		// Clear the pool for a clean test run.
		POOL.clear();

		let id = "127.0.0.1";
		let context = "/api/login";
		let limit = 5;

		// First 5 requests should be allowed.
		for _ in 0..limit {
			assert!(check(id, context, limit), "Request should be allowed");
		}

		// The 6th request should be denied.
		assert!(!check(id, context, limit), "Request should be denied");
	}

	#[tokio::test]
	#[serial]
	async fn test_rate_limiter_window_resets() {
		POOL.clear();

		let id = "user-a";
		let context = "file-upload";
		let limit = 2;

		// Two requests are allowed.
		assert!(check(id, context, limit));
		assert!(check(id, context, limit));
		// Third is blocked.
		assert!(!check(id, context, limit));

		// Wait for the 1-second window to expire.
		tokio::time::sleep(Duration::from_millis(1100)).await;

		// The limit should now be reset.
		assert!(
			check(id, context, limit),
			"Request should be allowed after window reset"
		);
	}

	#[tokio::test]
	#[serial]
	async fn test_rate_limiter_isolates_keys() {
		POOL.clear();

		let limit = 1;

		// Request from id1 in context1 is allowed.
		assert!(check("id1", "context1", limit));
		// The same request is now blocked.
		assert!(!check("id1", "context1", limit));

		// But requests from other IDs or contexts are not affected.
		assert!(
			check("id2", "context1", limit),
			"Different ID should be isolated"
		);
		assert!(
			check("id1", "context2", limit),
			"Different context should be isolated"
		);
	}

	#[tokio::test]
	#[serial]
	async fn test_zero_limit_always_denies() {
		POOL.clear();
		assert!(
			!check("any-id", "any-context", 0),
			"Limit of 0 should always deny"
		);
	}
}
