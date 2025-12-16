/* src/modules/certs/loader.rs */

use crate::common::{getconf, requirements::Result};
use crate::modules::certs::{arcswap, format};
use fancy_log::{LogLevel, log};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use x509_parser::prelude::{FromDer, parse_x509_pem};

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

/// Ensures that a valid 'default.crt' and 'default.key' exist in the certs directory.
/// If missing or expiring within 7 days, it generates a new self-signed pair.
fn ensure_default_certificate() {
	let config_dir = getconf::get_config_dir().join("certs");
	if !config_dir.exists() {
		if let Err(e) = fs::create_dir_all(&config_dir) {
			log(
				LogLevel::Error,
				&format!("✗ Failed to create certs dir: {}", e),
			);
			return;
		}
	}

	let cert_path = config_dir.join("default.crt");
	let key_path = config_dir.join("default.key");

	let mut should_generate = false;
	let mut reason = "Missing files";

	// 1. Check existence
	if !cert_path.exists() || !key_path.exists() {
		should_generate = true;
	} else {
		// 2. Check expiration
		match check_cert_expiration(&cert_path) {
			Ok(is_expiring) => {
				if is_expiring {
					should_generate = true;
					reason = "Expiring within 7 days";
				}
			}
			Err(e) => {
				log(
					LogLevel::Warn,
					&format!("⚠ Failed to parse default cert, force regenerating: {}", e),
				);
				should_generate = true;
				reason = "Invalid format";
			}
		}
	}

	// 3. Generate if needed
	if should_generate {
		log(
			LogLevel::Info,
			&format!(
				"⚙ Generating self-signed 'default' certificate ({})",
				reason
			),
		);
		if let Err(e) = generate_self_signed(&cert_path, &key_path) {
			log(
				LogLevel::Error,
				&format!("✗ Failed to generate default certificate: {}", e),
			);
		} else {
			log(
				LogLevel::Info,
				"✓ Default certificate generated/renewed successfully.",
			);
		}
	}
}

/// Checks if the certificate at the given path is expiring within 7 days.
fn check_cert_expiration(cert_path: &Path) -> Result<bool> {
	let content =
		fs::read(cert_path).map_err(|e| crate::common::requirements::Error::Io(e.to_string()))?;

	// Parse PEM
	let (_, pem) = parse_x509_pem(&content)
		.map_err(|e| crate::common::requirements::Error::Tls(format!("PEM parse error: {}", e)))?;

	// Parse X509
	let (_, x509) = x509_parser::certificate::X509Certificate::from_der(&pem.contents)
		.map_err(|e| crate::common::requirements::Error::Tls(format!("X509 parse error: {}", e)))?;

	let not_after = x509.validity.not_after.timestamp();
	let now = SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.unwrap()
		.as_secs() as i64;

	// 7 days in seconds
	let buffer_seconds = 7 * 24 * 60 * 60;

	if not_after - now < buffer_seconds {
		Ok(true)
	} else {
		Ok(false)
	}
}

/// Generates a self-signed certificate (localhost, 127.0.0.1) using rcgen.
fn generate_self_signed(cert_path: &Path, key_path: &Path) -> Result<()> {
	let subject_alt_names = vec!["localhost".to_string(), "127.0.0.1".to_string()];

	let certified_key = rcgen::generate_simple_self_signed(subject_alt_names)
		.map_err(|e| crate::common::requirements::Error::Tls(format!("rcgen error: {}", e)))?;

	let pem_cert = certified_key.cert.pem();
	// FIXED: Use `signing_key` field instead of `key_pair`
	let pem_key = certified_key.signing_key.serialize_pem();

	fs::write(cert_path, pem_cert)
		.map_err(|e| crate::common::requirements::Error::Io(e.to_string()))?;
	fs::write(key_path, pem_key)
		.map_err(|e| crate::common::requirements::Error::Io(e.to_string()))?;

	Ok(())
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
	ensure_default_certificate();
	scan_and_load_certs();
}
