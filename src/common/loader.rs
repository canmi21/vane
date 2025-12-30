/* src/common/loader.rs */

use fancy_log::{LogLevel, log};
use serde::de::DeserializeOwned;
use std::{fs, path::Path};
use validator::Validate;

const EXTENSIONS: [&str; 4] = ["toml", "yaml", "yml", "json"];

/// A trait to abstract the pre-processing of loaded configs before validation.
pub trait PreProcess {
	fn pre_process(&mut self) {}
	/// Allows the loader to inject context (like protocol name) before validation.
	fn set_context(&mut self, _context: &str) {}
}

/// Loads, parses, and validates a config file from a specific path.
/// This is the low-level function used by both Port and Resolver loaders.
pub fn load_file<T>(path: &Path, context: Option<&str>) -> Option<T>
where
	T: DeserializeOwned + Validate + PreProcess,
{
	let content = match fs::read_to_string(path) {
		Ok(c) => c,
		Err(e) => {
			log(
				LogLevel::Error,
				&format!("✗ Failed to read config file {}: {}", path.display(), e),
			);
			return None;
		}
	};

	let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");

	let config_result: Result<T, String> = match ext {
		"toml" => toml::from_str(&content).map_err(|e| e.to_string()),
		"yaml" | "yml" => serde_yaml::from_str(&content).map_err(|e| e.to_string()),
		"json" => serde_json::from_str(&content).map_err(|e| e.to_string()),
		_ => return None,
	};

	match config_result {
		Ok(mut config) => {
			if let Some(ctx) = context {
				config.set_context(ctx);
			}
			config.pre_process(); // Apply pre-processing
			match config.validate() {
				Ok(_) => Some(config),
				Err(e) => {
					log(
						LogLevel::Error,
						&format!("✗ Validation failed for {}: {}", path.display(), e),
					);
					None
				}
			}
		}
		Err(e) => {
			log(
				LogLevel::Error,
				&format!("✗ Failed to parse config file {}: {}", path.display(), e),
			);
			None
		}
	}
}

/// Loads, parses, and validates a config file for a given base path.
/// Looks for files with supported extensions in the base path.
/// e.g. base_path=".../tcp" looks for "tcp.toml", "tcp.json", etc.
pub fn load_config<T>(base_name: &str, base_path: &Path) -> Option<T>
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
				"✗ Found multiple config files for {}: {:?}. Deactivating.",
				base_name.to_uppercase(),
				found_files
			),
		);
		return None;
	}

	let config_path = found_files.first()?;
	load_file(config_path, Some(base_name))
}
