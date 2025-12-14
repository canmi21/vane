/* src/modules/certs/loader.rs */

use crate::common::getconf;
use crate::modules::certs::{arcswap, format};
use fancy_log::{LogLevel, log};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use tokio::sync::mpsc;

struct CertCandidate {
	crt: Option<PathBuf>,
	pem: Option<PathBuf>,
	key: Option<PathBuf>,
}

impl CertCandidate {
	fn new() -> Self {
		Self {
			crt: None,
			pem: None,
			key: None,
		}
	}
}

/// Scans the certs directory and applies the Keep-Last-Good strategy.
pub fn scan_and_load_certs() {
	let config_dir = getconf::get_config_dir().join("certs");
	if !config_dir.exists() {
		log(
			LogLevel::Warn,
			"⚠ Certs directory does not exist, skipping scan.",
		);
		return;
	}

	log(LogLevel::Debug, "⚙ Scanning certificates directory...");

	// 1. Start with a clone of the current state (Keep-Last-Good Base)
	let current_state = arcswap::CERT_REGISTRY.load();
	let mut new_state = current_state.as_ref().clone();
	let mut attempts = 0;
	let mut successes = 0;

	// 2. Identify potential pairs
	let entries = match fs::read_dir(&config_dir) {
		Ok(e) => e,
		Err(e) => {
			log(
				LogLevel::Error,
				&format!("✗ Failed to read certs dir: {}", e),
			);
			return;
		}
	};

	// Map: Stem -> Candidate Files
	let mut candidates: HashMap<String, CertCandidate> = HashMap::new();

	for entry in entries.flatten() {
		let path = entry.path();
		if !path.is_file() {
			continue;
		}

		// Logic: Extract extension, everything else is the stem (including internal dots)
		if let Some(filename) = path.file_name().and_then(|s| s.to_str()) {
			if let Some(dot_idx) = filename.rfind('.') {
				let stem = &filename[..dot_idx];
				let ext = &filename[dot_idx + 1..];

				let record = candidates
					.entry(stem.to_string())
					.or_insert_with(CertCandidate::new);

				match ext {
					"crt" => record.crt = Some(path.clone()),
					"pem" => record.pem = Some(path.clone()),
					"key" => record.key = Some(path.clone()),
					_ => {} // Ignore other files
				}
			}
		}
	}

	// 3. Filter and Validate
	for (id, candidate) in candidates {
		// Rule A: Conflict Check. If both .crt and .pem exist for the same stem, discard both.
		if candidate.crt.is_some() && candidate.pem.is_some() {
			log(
				LogLevel::Warn,
				&format!(
					"⚠ Ambiguous cert definition for [{}]: Found both .crt and .pem. Ignoring.",
					id
				),
			);
			continue;
		}

		// Rule B: Key Check. Must have a .key file.
		let key_path = match candidate.key {
			Some(p) => p,
			None => {
				// If we have a cert but no key, warn.
				if candidate.crt.is_some() || candidate.pem.is_some() {
					log(
						LogLevel::Warn,
						&format!(
							"⚠ Orphaned certificate for [{}]: Missing .key file. Ignoring.",
							id
						),
					);
				}
				continue;
			}
		};

		// Select the cert path
		let cert_path = candidate.crt.or(candidate.pem);

		if let Some(c_path) = cert_path {
			attempts += 1;
			match format::load_and_validate_pair(&c_path, &key_path) {
				Ok(certified_key) => {
					// Success: Insert (Overwrite if exists in memory)
					new_state.insert(id.clone(), certified_key);
					successes += 1;
					log(LogLevel::Debug, &format!("✓ Loaded Cert: {}", id));
				}
				Err(e) => {
					log(
						LogLevel::Error,
						&format!("✗ Invalid Cert Pair [{}]: {}", id, e),
					);
					if new_state.contains_key(&id) {
						log(
							LogLevel::Warn,
							&format!("⚠ Keeping previous valid version of [{}]", id),
						);
					}
				}
			}
		}
	}

	// 4. Commit Changes
	let total_certs = new_state.len();
	arcswap::update_registry(new_state);

	if attempts > 0 {
		log(
			LogLevel::Info,
			&format!(
				"✓ Certs Sync: {}/{} valid updates. Total loaded: {}",
				successes, attempts, total_certs
			),
		);
	} else if total_certs > 0 {
		log(
			LogLevel::Info,
			&format!("✓ Certs preserved. Total loaded: {}", total_certs),
		);
	}
}

/// Long-running task to listen for filesystem changes and trigger reloads.
pub async fn listen_for_updates(mut rx: mpsc::Receiver<()>) {
	while rx.recv().await.is_some() {
		log(
			LogLevel::Info,
			"↻ Certs configuration changed. Reloading...",
		);
		scan_and_load_certs();
	}
}

/// Initial loader called by bootstrap.
pub fn initialize() {
	scan_and_load_certs();
}
