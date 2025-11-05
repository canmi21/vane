/* engine/src/proxy/router/watch.rs */

use super::hotswap;
use crate::daemon::config;
use crate::modules::domain::entrance as domain_helper;
use dashmap::DashMap;
use fancy_log::{LogLevel, log};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use once_cell::sync::Lazy; // <-- ADD THIS
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::task::JoinHandle;

// Manages debouncing tasks for router reloads.
// Key: Domain name (e.g., "example.com")
// Value: The handle to the scheduled tokio task that will perform the reload.
// --- FIX: Use Lazy to initialize the static DashMap at runtime. ---
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

	// --- FIX: Clone domain for use inside the async task. ---
	let domain_clone = domain.clone();
	let new_task = tokio::spawn(async move {
		tokio::time::sleep(Duration::from_secs(2)).await;
		// The task is complete, so remove it from the map.
		DEBOUNCE_TASKS.remove(&domain_clone);
		hotswap::load_and_swap_router(&domain_clone).await;
	});

	// The original `domain` is moved here.
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
