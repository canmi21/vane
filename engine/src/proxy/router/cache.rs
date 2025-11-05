/* engine/src/proxy/router/cache.rs */

use super::structure::RouterNode;
use arc_swap::ArcSwap;
use dashmap::DashMap;
use once_cell::sync::Lazy;
use std::sync::Arc;

// The global, thread-safe cache for all domain routers.
// Key: Domain name (e.g., "example.com")
// Value: An atomically swappable Arc pointer to the parsed RouterNode.
// This is now correctly kept private to the module.
static ROUTER_CACHE: Lazy<DashMap<String, Arc<ArcSwap<RouterNode>>>> = Lazy::new(DashMap::new);

/// Retrieves a read-only guard to the current router for a given domain.
///
/// This is highly efficient as it's just loading an atomic pointer.
/// The returned `arc_swap::Guard` ensures that the router data is not dropped
/// while it's being accessed.
pub fn get_router(domain: &str) -> Option<arc_swap::Guard<Arc<RouterNode>>> {
	ROUTER_CACHE
		.get(domain)
		.map(|router_arc_swap| router_arc_swap.load())
}

/// Inserts or updates a router for a specific domain.
///
/// If the domain is new, it inserts a new `ArcSwap`.
/// If the domain already exists, it atomically swaps the pointer to the new router,
/// ensuring that in-flight requests can finish with the old router while new
/// requests will immediately use the new one.
pub fn insert_router(domain: &str, router: RouterNode) {
	let new_arc = Arc::new(router);
	if let Some(entry) = ROUTER_CACHE.get(domain) {
		// Domain exists, perform an atomic swap.
		entry.store(new_arc);
	} else {
		// Domain is new, insert a new ArcSwap instance.
		ROUTER_CACHE.insert(domain.to_string(), Arc::new(ArcSwap::from(new_arc)));
	}
}

/// Removes a router from the cache, for example when a domain is deleted.
pub fn remove_router(domain: &str) {
	ROUTER_CACHE.remove(domain);
}

// --- FIX: Add a public, test-only function to clear the cache. ---
/// Clears the entire router cache.
/// This function is only compiled and available in test environments.
#[cfg(test)]
pub fn clear_cache() {
	ROUTER_CACHE.clear();
}
