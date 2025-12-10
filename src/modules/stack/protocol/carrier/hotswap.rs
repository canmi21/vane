/* src/modules/stack/protocol/carrier/hotswap.rs */

use super::model::{RESOLVER_REGISTRY, ResolverConfig, SUPPORTED_UPGRADE_PROTOCOLS};
use crate::common::getconf;
use crate::modules::stack::transport::loader;
use dashmap::DashMap;
use fancy_log::{LogLevel, log};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Scans the 'resolver' config subdirectory for protocol configurations.
///
/// Enforces a strict "Zero Tolerance" rule for parallel configurations:
/// If multiple config files exist for the same protocol (e.g., tls.yaml AND tls.json),
/// the ENTIRE protocol is ignored/disabled, and an error is logged.
pub fn scan_resolver_config() -> DashMap<String, Arc<ResolverConfig>> {
	let resolver_dir = getconf::get_config_dir().join("resolver");
	let registry = DashMap::new();

	if !resolver_dir.exists() || !resolver_dir.is_dir() {
		return registry;
	}

	for &protocol in SUPPORTED_UPGRADE_PROTOCOLS {
		let mut found_files = Vec::new();
		let extensions = ["yaml", "yml", "json", "toml"];

		// 1. Scan for all possible config files for this protocol
		for ext in &extensions {
			let file_path = resolver_dir.join(format!("{}.{}", protocol, ext));
			if file_path.exists() {
				found_files.push(file_path);
			}
		}

		// 2. Strict Conflict Check
		if found_files.len() > 1 {
			// Collect file names for the error message
			let file_names: Vec<_> = found_files
				.iter()
				.filter_map(|p| p.file_name().and_then(|n| n.to_str()))
				.collect();

			log(
				LogLevel::Error,
				&format!(
					"✗ Configuration Conflict: Multiple config files found for protocol '{}': {:?}. This protocol will be DISABLED until the conflict is resolved.",
					protocol, file_names
				),
			);
			// Skip loading for this protocol entirely
			continue;
		}

		// 3. Load if exactly one file exists
		if let Some(file_path) = found_files.first() {
			// loader::load_file returns Option<T>, handling parse/validation errors internally.
			if let Some(config) = loader::load_file::<ResolverConfig>(file_path) {
				registry.insert(protocol.to_string(), Arc::new(config));
				log(
					LogLevel::Debug,
					&format!("⚙ Loaded resolver config for protocol: {}", protocol),
				);
			}
		}
	}

	registry
}

/// Listens for update signals and reloads the resolver registry.
pub async fn listen_for_updates(mut rx: mpsc::Receiver<()>) {
	while rx.recv().await.is_some() {
		log(
			LogLevel::Info,
			"➜ Resolver config change signal received, reloading...",
		);

		let new_registry = scan_resolver_config();

		// Atomic Swap
		// If a protocol had a conflict during scan, it will be missing from new_registry,
		// effectively disabling it at runtime (which is the desired safe behavior).
		RESOLVER_REGISTRY.store(Arc::new(new_registry));

		log(LogLevel::Info, "✓ Resolver configurations reloaded.");
	}
}
