/* src/lazycert/hotswap.rs */

use super::config::LazyCertConfig;
use crate::common::config::{file_loader, loader};
use fancy_log::{LogLevel, log};

/// Scan and load LazyCert config from config directory
/// Returns None if config file doesn't exist or is invalid (keeps old state)
pub async fn scan_lazycert_config() -> Option<LazyCertConfig> {
	let config_dir = file_loader::get_config_dir();
	let res: loader::LoadResult<LazyCertConfig> =
		loader::load_config("lazycert", &config_dir.join("lazycert")).await;

	match res {
		loader::LoadResult::Ok(config) => {
			log(
				LogLevel::Info,
				&format!("Loaded LazyCert config from {}", config_dir.display()),
			);
			Some(config)
		}
		loader::LoadResult::NotFound => {
			log(
				LogLevel::Debug,
				"LazyCert config file not found, integration disabled",
			);
			None
		}
		loader::LoadResult::Invalid => {
			log(
				LogLevel::Warn,
				"Invalid LazyCert config, keeping previous state",
			);
			None
		}
	}
}
