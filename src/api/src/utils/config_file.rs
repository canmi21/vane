/* src/api/utils/config_file.rs */

use serde::{Serialize, de::DeserializeOwned};
use std::path::{Path, PathBuf};
use tokio::fs;

const EXTENSIONS: &[&str] = &["json", "yaml", "yml", "toml"];

#[derive(Debug)]
pub enum ConfigFileResult<T> {
	NotFound,
	Single { path: PathBuf, format: String, content: T },
	Ambiguous { found: Vec<String> },
	Error(String),
}

/// Finds a config file with any supported extension at the given base path.
///
/// # Arguments
///
/// * `base_path` - The path without extension (e.g., "config/ports/[8080]/tcp")
///
pub async fn find_config<T>(base_path: &Path) -> ConfigFileResult<T>
where
	T: DeserializeOwned,
{
	let mut found_paths = Vec::new();
	let mut found_formats = Vec::new();

	for ext in EXTENSIONS {
		let path = base_path.with_extension(ext);
		if fs::metadata(&path).await.is_ok() {
			found_paths.push(path);
			found_formats.push(ext.to_string());
		}
	}

	if found_paths.is_empty() {
		return ConfigFileResult::NotFound;
	}

	if found_paths.len() > 1 {
		return ConfigFileResult::Ambiguous { found: found_formats };
	}

	let path = found_paths.remove(0);
	let format = found_formats.remove(0);

	let content_str = match fs::read_to_string(&path).await {
		Ok(s) => s,
		Err(e) => return ConfigFileResult::Error(e.to_string()),
	};

	let content: T = match format.as_str() {
		"json" => match serde_json::from_str(&content_str) {
			Ok(c) => c,
			Err(e) => return ConfigFileResult::Error(format!("JSON error: {e}")),
		},
		"yaml" | "yml" => match serde_yaml::from_str(&content_str) {
			Ok(c) => c,
			Err(e) => return ConfigFileResult::Error(format!("YAML error: {e}")),
		},
		"toml" => match toml::from_str(&content_str) {
			Ok(c) => c,
			Err(e) => return ConfigFileResult::Error(format!("TOML error: {e}")),
		},
		_ => return ConfigFileResult::Error("Unsupported format".to_owned()),
	};

	ConfigFileResult::Single { path, format, content }
}

/// Deletes all config files with any supported extension at the given base path.
///
/// Returns `true` if any file was deleted.
pub async fn delete_all_formats(base_path: &Path) -> std::io::Result<bool> {
	let mut deleted = false;
	for ext in EXTENSIONS {
		let path = base_path.with_extension(ext);
		if fs::metadata(&path).await.is_ok() {
			fs::remove_file(&path).await?;
			deleted = true;
		}
	}
	Ok(deleted)
}

/// Writes content to a .json file at the given base path.
///
/// Returns the path to the written file.
pub async fn write_json<T: Serialize>(base_path: &Path, content: &T) -> std::io::Result<PathBuf> {
	let path = base_path.with_extension("json");
	let json_str = serde_json::to_string_pretty(content)?;
	fs::write(&path, json_str).await?;
	Ok(path)
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde::{Deserialize, Serialize};
	use tempfile::tempdir;

	#[derive(Serialize, Deserialize, Debug, PartialEq)]
	struct TestConfig {
		name: String,
		value: i32,
	}

	#[tokio::test]
	async fn test_find_config_not_found() {
		let dir = tempdir().unwrap();
		let base_path = dir.path().join("test");

		let result: ConfigFileResult<TestConfig> = find_config(&base_path).await;
		assert!(matches!(result, ConfigFileResult::NotFound));
	}

	#[tokio::test]
	async fn test_write_and_find_json() {
		let dir = tempdir().unwrap();
		let base_path = dir.path().join("test");
		let config = TestConfig { name: "foo".into(), value: 42 };

		let written_path = write_json(&base_path, &config).await.unwrap();
		assert!(written_path.ends_with("test.json"));

		let result: ConfigFileResult<TestConfig> = find_config(&base_path).await;
		match result {
			ConfigFileResult::Single { path, format, content } => {
				assert_eq!(path, written_path);
				assert_eq!(format, "json");
				assert_eq!(content, config);
			}
			_ => panic!("Expected Single result"),
		}
	}

	#[tokio::test]
	async fn test_ambiguous_config() {
		let dir = tempdir().unwrap();
		let base_path = dir.path().join("test");

		fs::write(base_path.with_extension("json"), "{}").await.unwrap();
		fs::write(base_path.with_extension("yaml"), "").await.unwrap();

		let result: ConfigFileResult<TestConfig> = find_config(&base_path).await;
		match result {
			ConfigFileResult::Ambiguous { found } => {
				assert!(found.contains(&"json".to_string()));
				assert!(found.contains(&"yaml".to_string()));
			}
			_ => panic!("Expected Ambiguous result"),
		}
	}

	#[tokio::test]
	async fn test_delete_all() {
		let dir = tempdir().unwrap();
		let base_path = dir.path().join("test");

		fs::write(base_path.with_extension("json"), "{}").await.unwrap();
		fs::write(base_path.with_extension("yaml"), "").await.unwrap();

		let deleted = delete_all_formats(&base_path).await.unwrap();
		assert!(deleted);

		let result: ConfigFileResult<TestConfig> = find_config(&base_path).await;
		assert!(matches!(result, ConfigFileResult::NotFound));
	}
}
