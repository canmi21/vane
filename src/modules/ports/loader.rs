/* src/modules/ports/loader.rs */

use crate::modules::server::l4::model::{TcpConfig, UdpConfig};
use fancy_log::{LogLevel, log};
use serde::de::DeserializeOwned;
use std::{fs, path::Path};
use validator::Validate;

// Removed "ron" (and "vane") from the list of supported extensions.
const EXTENSIONS: [&str; 3] = ["toml", "yaml", "json"];

/// A trait to abstract the pre-processing of loaded configs before validation.
pub trait PreProcess {
	fn pre_process(&mut self);
}

impl PreProcess for TcpConfig {
	fn pre_process(&mut self) {
		for rule in &mut self.rules {
			rule.name = rule.name.to_lowercase();
		}
	}
}

impl PreProcess for UdpConfig {
	fn pre_process(&mut self) {
		for rule in &mut self.rules {
			rule.name = rule.name.to_lowercase();
		}
	}
}

/// Loads, parses, and validates a config file for a given protocol and port.
pub fn load_config<T>(port: u16, protocol_name: &str, base_path: &Path) -> Option<T>
where
	T: DeserializeOwned + Validate + PreProcess,
{
	let mut found_files = Vec::new();
	for ext in EXTENSIONS {
		let path = base_path.with_extension(ext);
		if path.exists() {
			found_files.push(path);
		}
	}

	if found_files.len() > 1 {
		log(
			LogLevel::Warn,
			&format!(
				"✗ Found multiple config files for {} on port {}: {:?}. Deactivating.",
				protocol_name.to_uppercase(),
				port,
				found_files
			),
		);
		return None;
	}

	let config_path = found_files.first()?;
	let content = match fs::read_to_string(config_path) {
		Ok(c) => c,
		Err(e) => {
			log(
				LogLevel::Error,
				&format!(
					"✗ Failed to read config file {}: {}",
					config_path.display(),
					e
				),
			);
			return None;
		}
	};

	let ext = config_path
		.extension()
		.and_then(|s| s.to_str())
		.unwrap_or("");

	// Removed the match arm for "ron" / "vane".
	let config_result: Result<T, String> = match ext {
		"toml" => toml::from_str(&content).map_err(|e| e.to_string()),
		"yaml" => serde_yaml::from_str(&content).map_err(|e| e.to_string()),
		"json" => serde_json::from_str(&content).map_err(|e| e.to_string()),
		_ => return None,
	};

	match config_result {
		Ok(mut config) => {
			config.pre_process();
			match config.validate() {
				Ok(_) => Some(config),
				Err(e) => {
					log(
						LogLevel::Error,
						&format!("✗ Validation failed for {}: {}", config_path.display(), e),
					);
					None
				}
			}
		}
		Err(e) => {
			log(
				LogLevel::Error,
				&format!(
					"✗ Failed to parse config file {}: {}",
					config_path.display(),
					e
				),
			);
			None
		}
	}
}
