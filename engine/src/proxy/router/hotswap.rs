/* engine/src/proxy/router/hotswap.rs */

use super::{cache, structure::RouterNode};
use crate::daemon::config;
use crate::proxy::domain::handler as domain_helper;
use fancy_log::{LogLevel, log};

/// Reads a `router.gen` file from disk, parses it, and atomically swaps it into the cache.
pub async fn load_and_swap_router(domain: &str) {
	let router_path = config::get_config_dir()
		.join(domain_helper::domain_to_dir_name(domain))
		.join("router.gen");

	if !router_path.exists() {
		log(
			LogLevel::Debug,
			&format!(
				"No router.gen for '{}', removing from cache if present.",
				domain
			),
		);
		cache::remove_router(domain);
		return;
	}

	match tokio::fs::read_to_string(&router_path).await {
		Ok(content) => {
			if content.trim().is_empty() {
				log(
					LogLevel::Warn,
					&format!("Router config for '{}' is empty. Unloading.", domain),
				);
				cache::remove_router(domain);
				return;
			}

			match serde_json::from_str::<RouterNode>(&content) {
				Ok(router_node) => {
					log(
						LogLevel::Info,
						&format!("Reloading router for '{}'.", domain),
					);
					cache::insert_router(domain, router_node);
				}
				Err(e) => {
					log(
						LogLevel::Error,
						&format!("Failed to parse router.gen for '{}': {}", domain, e),
					);
				}
			}
		}
		Err(e) => {
			log(
				LogLevel::Error,
				&format!("Failed to read router.gen for '{}': {}", domain, e),
			);
		}
	}
}
