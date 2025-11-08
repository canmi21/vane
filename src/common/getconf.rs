/* src/common/getconf.rs */

use crate::common::getenv;
use fancy_log::{LogLevel, log};
use std::fs;
use std::path::PathBuf;

/// Retrieves the configuration directory path.
///
/// It first checks the `CONFIG_DIR` environment variable. If not set,
/// it falls back to "~/vane/". The path is expanded to handle `~`.
pub fn get_config_dir() -> PathBuf {
	let path_str = getenv::get_env("CONFIG_DIR", "~/vane/".to_string());
	let expanded_path = shellexpand::tilde(&path_str).to_string();
	PathBuf::from(expanded_path)
}

/// Initializes required configuration files.
///
/// It ensures the main config directory exists, then checks for specific
/// config files, creating empty ones if they are missing.
pub fn init_config_files(files_to_check: Vec<&str>) {
	let config_dir = get_config_dir();

	// First, ensure the main configuration directory exists.
	if !config_dir.exists() {
		if let Err(e) = fs::create_dir_all(&config_dir) {
			log(
				LogLevel::Error,
				&format!(
					"Failed to create main config directory {}: {}",
					config_dir.display(),
					e
				),
			);
			return; // Can't proceed if the base directory fails to be created.
		}
	}

	for file_path in files_to_check {
		let full_path = config_dir.join(file_path);

		if full_path.exists() {
			continue;
		}

		// Ensure the parent directory for the specific file exists. This handles
		// subdirectories within config_dir like 'etc/'.
		if let Some(parent_dir) = full_path.parent() {
			if let Err(e) = fs::create_dir_all(parent_dir) {
				log(
					LogLevel::Error,
					&format!(
						"Failed to create config subdirectory {}: {}",
						parent_dir.display(),
						e
					),
				);
				continue; // Skip to the next file on error.
			}
		}

		// Create an empty file.
		match fs::File::create(&full_path) {
			Ok(_) => log(
				LogLevel::Debug,
				&format!("Created default config file at: {}", full_path.display()),
			),
			Err(e) => log(
				LogLevel::Error,
				&format!(
					"Failed to create config file {}: {}",
					full_path.display(),
					e
				),
			),
		}
	}
}
