/* src/common/loader.rs */

use crate::common::getconf;

use fancy_log::{LogLevel, log};

use serde::de::DeserializeOwned;

use std::{fs, path::Path};

use validator::Validate;

const EXTENSIONS: [&str; 4] = ["toml", "yaml", "yml", "json"];

/// The result of a configuration load attempt.

pub enum LoadResult<T> {
	/// Configuration was successfully loaded and validated.
	Ok(T),

	/// No configuration file was found at the expected location.
	NotFound,

	/// A configuration file exists but failed to parse or validate.
	Invalid,
}

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
	// 1. Security Check: Path Canonicalization

	// Ensure the path is within the trusted CONFIG_DIR

	let config_dir = getconf::get_config_dir();

	let root = fs::canonicalize(&config_dir).unwrap_or(config_dir);

	let absolute_path = match fs::canonicalize(path) {
		Ok(p) => p,

		Err(e) => {
			log(
				LogLevel::Error,
				&format!("✗ Failed to resolve config path {}: {}", path.display(), e),
			);

			return None;
		}
	};

	if !absolute_path.starts_with(&root) {
		log(
			LogLevel::Error,
			&format!(
				"✗ Security Violation: Config path {} is outside the configuration directory.",
				path.display()
			),
		);

		return None;
	}

	let content = match fs::read_to_string(&absolute_path) {
		Ok(c) => c,

		Err(e) => {
			log(
				LogLevel::Error,
				&format!(
					"✗ Failed to read config file {}: {}",
					absolute_path.display(),
					e
				),
			);

			return None;
		}
	};

	let ext = absolute_path
		.extension()
		.and_then(|s| s.to_str())
		.unwrap_or("");

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
						&format!("✗ Validation failed for {}: {}", absolute_path.display(), e),
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
					absolute_path.display(),
					e
				),
			);

			None
		}
	}
}

/// Loads, parses, and validates a config file for a given base path.
/// Looks for files with supported extensions in the base path.
/// e.g. base_path=".../tcp" looks for "tcp.toml", "tcp.json", etc.
pub fn load_config<T>(base_name: &str, base_path: &Path) -> LoadResult<T>
where
	T: DeserializeOwned + Validate + PreProcess,
{
	let mut found_content = None;
	let mut matched_path = None;

	for ext in EXTENSIONS {
		let path = base_path.with_extension(ext);
		// Directly attempt to read to avoid TOCTOU race between exists() and read()
		if let Ok(content) = fs::read_to_string(&path) {
			if found_content.is_some() {
				log(
					LogLevel::Warn,
					&format!(
						"✗ Found multiple config files for {}. Already loaded one. Ignoring: {}",
						base_name.to_uppercase(),
						path.display()
					),
				);
				continue;
			}
			found_content = Some(content);
			matched_path = Some(path);
		}
	}

	match matched_path {
		Some(path) => match load_file(&path, Some(base_name)) {
			Some(config) => LoadResult::Ok(config),
			None => LoadResult::Invalid,
		},
		None => LoadResult::NotFound,
	}
}
