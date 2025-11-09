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

	if !config_dir.exists() {
		if let Err(e) = fs::create_dir_all(&config_dir) {
			log(
				LogLevel::Error,
				&format!(
					"✗ Failed to create main config directory {}: {}",
					config_dir.display(),
					e
				),
			);
			return;
		}
	}

	for file_path in files_to_check {
		let full_path = config_dir.join(file_path);

		if full_path.exists() {
			continue;
		}

		if let Some(parent_dir) = full_path.parent() {
			if let Err(e) = fs::create_dir_all(parent_dir) {
				log(
					LogLevel::Error,
					&format!(
						"✗ Failed to create config subdirectory {}: {}",
						parent_dir.display(),
						e
					),
				);
				continue;
			}
		}

		match fs::File::create(&full_path) {
			Ok(_) => log(
				LogLevel::Debug,
				&format!("⚙ Created default config file at: {}", full_path.display()),
			),
			Err(e) => log(
				LogLevel::Error,
				&format!(
					"✗ Failed to create config file {}: {}",
					full_path.display(),
					e
				),
			),
		}
	}
}

/// Ensures a list of subdirectories exists within the main configuration directory.
///
/// For each directory name provided, it checks if the directory exists and creates
/// it if it does not.
pub fn init_config_dirs(dir_names: Vec<&str>) {
	let config_dir = get_config_dir();
	for dir_name in dir_names {
		let full_path = config_dir.join(dir_name);

		if full_path.exists() {
			if !full_path.is_dir() {
				log(
					LogLevel::Error,
					&format!(
						"✗ Path {} exists but is not a directory.",
						full_path.display()
					),
				);
			}
			continue;
		}

		match fs::create_dir_all(&full_path) {
			Ok(_) => log(
				LogLevel::Debug,
				&format!(
					"⚙ Created default config directory at: {}",
					full_path.display()
				),
			),
			Err(e) => log(
				LogLevel::Error,
				&format!(
					"✗ Failed to create config directory {}: {}",
					full_path.display(),
					e
				),
			),
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use dirs;
	use serial_test::serial;
	use temp_env;
	use tempfile::tempdir;

	/// Tests that the config dir is correctly retrieved from the environment variable.
	#[test]
	#[serial]
	fn test_get_config_dir_from_env() {
		let temp_dir = tempdir().unwrap();
		let temp_path_str = temp_dir.path().to_str().unwrap();

		temp_env::with_var("CONFIG_DIR", Some(temp_path_str), || {
			assert_eq!(get_config_dir(), temp_dir.path());
		});
	}

	/// Tests that the config dir falls back to the default when the env var is not set.
	#[test]
	#[serial]
	fn test_get_config_dir_default_fallback() {
		temp_env::with_var_unset("CONFIG_DIR", || {
			let home_dir = dirs::home_dir().unwrap();
			let expected_path = home_dir.join("vane/");
			assert_eq!(get_config_dir(), expected_path);
		});
	}

	/// Tests the creation of specified configuration files and their subdirectories.
	#[test]
	#[serial]
	fn test_init_config_files_creation() {
		let temp_dir = tempdir().unwrap();
		let temp_path = temp_dir.path();

		temp_env::with_var("CONFIG_DIR", Some(temp_path.to_str().unwrap()), || {
			let files_to_create = vec!["nodes.toml", "listener/80/tcp.yaml"];
			init_config_files(files_to_create);

			assert!(temp_path.join("nodes.toml").exists());
			assert!(temp_path.join("nodes.toml").is_file());
			assert!(temp_path.join("listener/80/tcp.yaml").exists());
			assert!(temp_path.join("listener/80/tcp.yaml").is_file());
		});
	}

	/// Tests the creation of specified configuration directories.
	#[test]
	#[serial]
	fn test_init_config_dirs_creation() {
		let temp_dir = tempdir().unwrap();
		let temp_path = temp_dir.path();

		temp_env::with_var("CONFIG_DIR", Some(temp_path.to_str().unwrap()), || {
			let dirs_to_create = vec!["listener", "ssl_certs"];
			init_config_dirs(dirs_to_create);

			assert!(temp_path.join("listener").exists());
			assert!(temp_path.join("listener").is_dir());
			assert!(temp_path.join("ssl_certs").exists());
			assert!(temp_path.join("ssl_certs").is_dir());
		});
	}
}
