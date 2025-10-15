/* engine/src/daemon/config.rs */

use fancy_log::{LogLevel, log};
use std::env;
use std::fs;
use std::path::PathBuf;

/// Initializes the configuration directories.
/// Creates required subdirectories inside the path specified by CONFIG_DIR,
/// or "~/vane/" if the variable is not set.
pub fn initialize_config_directory() {
	// Read the CONFIG_DIR environment variable, defaulting to "~/vane/" if not set.
	let config_dir = env::var("CONFIG_DIR").unwrap_or_else(|_| "~/vane/".to_string());

	// Expand tilde (~) in the path to the user's home directory.
	let expanded_path_str = shellexpand::tilde(&config_dir).to_string();
	let base_path = PathBuf::from(expanded_path_str);

	// Define the subdirectories to be created.
	let dirs_to_create = ["[fallback]", "templates", "logs", "certs"];

	for dir_name in dirs_to_create.iter() {
		let mut path = base_path.clone();
		path.push(dir_name);

		// If the directory does not exist, create it.
		if !path.exists() {
			match fs::create_dir_all(&path) {
				Ok(_) => {
					// Log only on successful creation.
					log(LogLevel::Debug, &format!("+ Created {:?}", path));
				}
				Err(e) => {
					// Log an error if creation fails.
					log(
						LogLevel::Error,
						&format!("! Failed to create directory {:?}: {}", path, e),
					);
				}
			}
		}
		// If the directory already exists, do nothing.
	}
}

/// Returns the application's configuration directory path.
/// It prioritizes the `CONFIG_DIR` environment variable and falls back to `~/vane`.
pub fn get_config_dir() -> PathBuf {
	// Read the CONFIG_DIR environment variable, defaulting to "~/vane/" if not set.
	let config_dir = env::var("CONFIG_DIR").unwrap_or_else(|_| "~/vane".to_string());
	// Expand tilde (~) in the path to the user's home directory.
	let expanded_path_str = shellexpand::tilde(&config_dir).to_string();
	PathBuf::from(expanded_path_str)
}

/// Returns the full path to the `origins.json` file.
pub fn get_origins_path() -> PathBuf {
	get_config_dir().join("origins.json")
}

/// Returns the full path to the `origin_monitor.json` file.
pub fn get_monitor_config_path() -> PathBuf {
	get_config_dir().join("origin_monitor.json")
}
