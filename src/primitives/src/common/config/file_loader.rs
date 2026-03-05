/* src/common/config/file_loader.rs */

use fancy_log::{LogLevel, log};
use std::path::PathBuf;
use tokio::fs;

#[cfg(not(windows))]
const DEFAULT_CONFIG_DIR: &str = "/etc/vane/";

#[cfg(windows)]
const DEFAULT_CONFIG_DIR: &str = r"C:\ProgramData\Vane\";

/// Retrieves the configuration directory path.
#[must_use]
pub fn get_config_dir() -> PathBuf {
	let path_str = envflag::get_string("CONFIG_DIR", DEFAULT_CONFIG_DIR);
	let expanded_path = shellexpand::tilde(&path_str).to_string();
	PathBuf::from(expanded_path)
}

/// Initializes required configuration files.
pub async fn init_config_files(files_to_check: Vec<&str>) {
	let config_dir = get_config_dir();

	if fs::metadata(&config_dir).await.is_err()
		&& let Err(e) = fs::create_dir_all(&config_dir).await
	{
		log(
			LogLevel::Error,
			&format!("✗ Failed to create main config directory {}: {}", config_dir.display(), e),
		);
		return;
	}

	for file_path in files_to_check {
		let full_path = config_dir.join(file_path);

		if fs::metadata(&full_path).await.is_ok() {
			continue;
		}

		if let Some(parent_dir) = full_path.parent()
			&& fs::metadata(parent_dir).await.is_err()
			&& let Err(e) = fs::create_dir_all(parent_dir).await
		{
			log(
				LogLevel::Error,
				&format!("✗ Failed to create config subdirectory {}: {}", parent_dir.display(), e),
			);
			continue;
		}

		match fs::File::create(&full_path).await {
			Ok(_) => {
				log(LogLevel::Debug, &format!("⚙ Created default config file at: {}", full_path.display()))
			}
			Err(e) => log(
				LogLevel::Error,
				&format!("✗ Failed to create config file {}: {}", full_path.display(), e),
			),
		}
	}
}

/// Ensures a list of subdirectories exists.
pub async fn init_config_dirs(dir_names: Vec<&str>) {
	let config_dir = get_config_dir();
	for dir_name in dir_names {
		let full_path = config_dir.join(dir_name);

		if let Ok(metadata) = fs::metadata(&full_path).await {
			if !metadata.is_dir() {
				log(
					LogLevel::Error,
					&format!("✗ Path {} exists but is not a directory.", full_path.display()),
				);
			}
			continue;
		}

		match fs::create_dir_all(&full_path).await {
			Ok(_) => log(
				LogLevel::Debug,
				&format!("⚙ Created default config directory at: {}", full_path.display()),
			),
			Err(e) => log(
				LogLevel::Error,
				&format!("✗ Failed to create config directory {}: {}", full_path.display(), e),
			),
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_tilde_expansion() {
		let expanded = shellexpand::tilde("~/config").to_string();
		assert!(!expanded.starts_with('~'));
	}

	#[test]
	fn test_default_config_dir_value() {
		#[cfg(not(windows))]
		assert_eq!(DEFAULT_CONFIG_DIR, "/etc/vane/");
		#[cfg(windows)]
		assert_eq!(DEFAULT_CONFIG_DIR, r"C:\ProgramData\Vane\");
	}
}
