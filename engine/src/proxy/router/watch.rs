/* engine/src/proxy/router/watch.rs */

use super::hotswap;
use crate::daemon::config;
use crate::proxy::domain::handler as domain_helper;
use dashmap::DashMap;
use fancy_log::{LogLevel, log};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use once_cell::sync::Lazy;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::task::JoinHandle;

// Manages debouncing tasks for router reloads.
// Key: Domain name (e.g., "example.com")
// Value: The handle to the scheduled tokio task that will perform the reload.
static DEBOUNCE_TASKS: Lazy<DashMap<String, JoinHandle<()>>> = Lazy::new(DashMap::new);

/// The main entry point to start the file system watcher.
/// It spawns a background task to monitor the config directory for changes.
pub fn start_router_watcher() {
	let config_dir = config::get_config_dir();
	tokio::spawn(async move {
		if let Err(e) = watch_config_directory(config_dir).await {
			log(LogLevel::Error, &format!("Router watcher failed: {}", e));
		}
	});
}

/// Initializes and runs the file system watcher loop.
async fn watch_config_directory(path: PathBuf) -> notify::Result<()> {
	// We use an async channel to receive events from the watcher.
	let (tx, mut rx) = tokio::sync::mpsc::channel(1);

	let mut watcher = RecommendedWatcher::new(
		move |res| {
			if let Ok(event) = res {
				// Send events to the async task for processing.
				tx.blocking_send(event).expect("Failed to send fs event");
			}
		},
		notify::Config::default(),
	)?;

	watcher.watch(&path, RecursiveMode::Recursive)?;
	log(
		LogLevel::Info,
		&format!("Router watcher started on: {:?}", path),
	);

	// The async event processing loop.
	while let Some(event) = rx.recv().await {
		process_event(event);
	}

	Ok(())
}

/// Analyzes a file system event and triggers the debounced reload if necessary.
fn process_event(event: Event) {
	// We only care about file changes, creations, or removals.
	if !matches!(
		event.kind,
		notify::EventKind::Modify(_) | notify::EventKind::Create(_) | notify::EventKind::Remove(_)
	) {
		return;
	}

	for path in &event.paths {
		// Check if the modified file is a `router.gen`.
		if path.file_name().and_then(|s| s.to_str()) == Some("router.gen") {
			if let Some(domain) = extract_domain_from_path(path) {
				schedule_reload(domain);
			}
		}
		// Also check for domain directory removals.
		if event.kind.is_remove() {
			if let Some(domain) = extract_domain_from_path_if_dir(path) {
				log(
					LogLevel::Debug,
					&format!(
						"Domain directory for '{}' removed, unloading router.",
						&domain
					),
				);
				schedule_reload(domain); // will trigger a reload which finds no file
			}
		}
	}
}

/// Schedules a router reload for a domain with a 2-second debounce.
fn schedule_reload(domain: String) {
	// If a reload is already scheduled for this domain, cancel it.
	if let Some((_, old_task)) = DEBOUNCE_TASKS.remove(&domain) {
		old_task.abort();
	}

	log(
		LogLevel::Debug,
		&format!("Scheduling router reload for '{}' in 2s.", domain),
	);

	let domain_clone = domain.clone();
	let new_task = tokio::spawn(async move {
		tokio::time::sleep(Duration::from_secs(2)).await;
		// The task is complete, so remove it from the map.
		DEBOUNCE_TASKS.remove(&domain_clone);
		hotswap::load_and_swap_router(&domain_clone).await;
	});

	DEBOUNCE_TASKS.insert(domain, new_task);
}

/// Extracts the domain name from a path like `.../[domain.com]/router.gen`.
fn extract_domain_from_path(path: &Path) -> Option<String> {
	path
		.parent()
		.and_then(|p| p.file_name())
		.and_then(|s| s.to_str())
		.and_then(domain_helper::dir_name_to_domain)
}

/// Extracts domain name if path is a directory like `.../[domain.com]`.
fn extract_domain_from_path_if_dir(path: &Path) -> Option<String> {
	path
		.file_name()
		.and_then(|s| s.to_str())
		.and_then(domain_helper::dir_name_to_domain)
}

/// Scans all domain directories and loads their routers on startup.
pub async fn initial_load_all_routers() {
	log(
		LogLevel::Info,
		"Performing initial load of all domain routers...",
	);
	let domains = domain_helper::list_domains_internal().await;
	for domain in domains {
		hotswap::load_and_swap_router(&domain).await;
	}
	log(LogLevel::Info, "Initial router load complete.");
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::proxy::router::cache; // <-- No longer need to import ROUTER_CACHE
	use serial_test::serial;
	use std::env;
	use tempfile::tempdir;

	#[tokio::test]
	#[serial]
	async fn test_initial_load_all_routers_success() {
		// Setup: Create an isolated environment
		let tmp_dir = tempdir().unwrap();
		let config_dir = tmp_dir.path();
		let original_env = env::var("CONFIG_DIR").ok();
		unsafe {
			env::set_var("CONFIG_DIR", config_dir.to_str().unwrap());
		}
		cache::clear_cache(); // FIX: Use the public test function

		// Create a domain with a valid router
		let domain1_dir = config_dir.join("[example.com]");
		tokio::fs::create_dir(&domain1_dir).await.unwrap();
		tokio::fs::write(
			domain1_dir.join("router.gen"),
			r#"{"type": "test-node", "data": {}}"#,
		)
		.await
		.unwrap();

		// Create another domain with no router file
		let domain2_dir = config_dir.join("[no-router.com]");
		tokio::fs::create_dir(&domain2_dir).await.unwrap();

		// Action: Run the initial loader
		initial_load_all_routers().await;

		// Assert: Check the cache state via public API
		assert!(
			cache::get_router("example.com").is_some(),
			"Router for example.com should be loaded"
		);
		assert!(
			cache::get_router("no-router.com").is_none(),
			"Domain with no router file should not be in cache"
		);

		// Teardown
		if let Some(orig) = original_env {
			unsafe { env::set_var("CONFIG_DIR", orig) };
		} else {
			unsafe { env::remove_var("CONFIG_DIR") };
		}
		cache::clear_cache();
	}

	#[tokio::test]
	#[serial]
	async fn test_load_and_swap_router_lifecycle() {
		// Setup
		let tmp_dir = tempdir().unwrap();
		let config_dir = tmp_dir.path();
		let original_env = env::var("CONFIG_DIR").ok();
		unsafe {
			env::set_var("CONFIG_DIR", config_dir.to_str().unwrap());
		}
		cache::clear_cache();

		let domain_dir = config_dir.join("[test.com]");
		tokio::fs::create_dir(&domain_dir).await.unwrap();
		let router_path = domain_dir.join("router.gen");

		// 1. Create and Load
		tokio::fs::write(&router_path, r#"{"type": "v1", "data": {}}"#)
			.await
			.unwrap();
		hotswap::load_and_swap_router("test.com").await;
		let router_v1 = cache::get_router("test.com").unwrap();
		// FIX: Access field directly on the Guard via auto-deref.
		assert_eq!(router_v1.node_type, "v1");

		// 2. Update and Swap
		tokio::fs::write(&router_path, r#"{"type": "v2", "data": {}}"#)
			.await
			.unwrap();
		hotswap::load_and_swap_router("test.com").await;
		let router_v2 = cache::get_router("test.com").unwrap();
		assert_eq!(
			// FIX: Access field directly on the Guard via auto-deref.
			router_v2.node_type,
			"v2",
			"Router should be atomically swapped to v2"
		);

		// 3. Delete file and Unload
		tokio::fs::remove_file(&router_path).await.unwrap();
		hotswap::load_and_swap_router("test.com").await;
		assert!(
			cache::get_router("test.com").is_none(),
			"Router should be removed from cache after file deletion"
		);

		// Teardown
		if let Some(orig) = original_env {
			unsafe { env::set_var("CONFIG_DIR", orig) };
		} else {
			unsafe { env::remove_var("CONFIG_DIR") };
		}
		cache::clear_cache();
	}

	#[tokio::test]
	#[serial]
	async fn test_load_router_handles_invalid_and_empty_files() {
		// Setup
		let tmp_dir = tempdir().unwrap();
		let config_dir = tmp_dir.path();
		let original_env = env::var("CONFIG_DIR").ok();
		unsafe {
			env::set_var("CONFIG_DIR", config_dir.to_str().unwrap());
		}
		cache::clear_cache();
		let domain_dir = config_dir.join("[bad.com]");
		tokio::fs::create_dir(&domain_dir).await.unwrap();
		let router_path = domain_dir.join("router.gen");

		// 1. Load with invalid JSON
		tokio::fs::write(&router_path, r#"{"type": "bad" "#)
			.await
			.unwrap();
		hotswap::load_and_swap_router("bad.com").await;
		assert!(
			cache::get_router("bad.com").is_none(),
			"Invalid JSON should not be loaded into cache"
		);

		// 2. Load with empty content
		tokio::fs::write(&router_path, "   ").await.unwrap(); // Whitespace only
		hotswap::load_and_swap_router("bad.com").await;
		assert!(
			cache::get_router("bad.com").is_none(),
			"Empty file should not be loaded into cache"
		);

		// Teardown
		if let Some(orig) = original_env {
			unsafe { env::set_var("CONFIG_DIR", orig) };
		} else {
			unsafe { env::remove_var("CONFIG_DIR") };
		}
		cache::clear_cache();
	}
}
