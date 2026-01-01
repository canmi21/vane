/* src/common/loader.rs */

use crate::common::getconf;
use fancy_log::{LogLevel, log};
use serde::de::DeserializeOwned;
use std::path::Path;
use tokio::fs;
use validator::Validate;

const EXTENSIONS: [&str; 4] = ["toml", "yaml", "yml", "json"];

pub enum LoadResult<T> {
	Ok(T),
	NotFound,
	Invalid,
}

pub trait PreProcess {
	fn pre_process(&mut self) {}
	fn set_context(&mut self, _context: &str) {}
}

pub async fn load_file<T>(path: &Path, context: Option<&str>) -> Option<T>
where
	T: DeserializeOwned + Validate + PreProcess,
{
	let config_dir = getconf::get_config_dir();
	let root = fs::canonicalize(&config_dir)
		.await
		.unwrap_or_else(|_| config_dir.clone());

	let absolute_path = match fs::canonicalize(path).await {
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
				"✗ Security Violation: Config path {} is outside directory.",
				path.display()
			),
		);
		return None;
	}

	let content = match fs::read_to_string(&absolute_path).await {
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
			config.pre_process();
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

pub async fn load_config<T>(base_name: &str, base_path: &Path) -> LoadResult<T>
where
	T: DeserializeOwned + Validate + PreProcess,
{
	let mut matched_path = None;

	for ext in EXTENSIONS {
		let path = base_path.with_extension(ext);
		if fs::metadata(&path).await.is_ok() {
			if matched_path.is_some() {
				log(
					LogLevel::Warn,
					&format!(
						"✗ Multiple config files for {}. Ignoring: {}",
						base_name,
						path.display()
					),
				);
				continue;
			}
			matched_path = Some(path);
		}
	}

	match matched_path {
		Some(path) => match load_file(&path, Some(base_name)).await {
			Some(config) => LoadResult::Ok(config),
			None => LoadResult::Invalid,
		},
		None => LoadResult::NotFound,
	}
}
