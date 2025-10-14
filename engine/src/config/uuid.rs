/* engine/src/config/uuid.rs */

use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use chrono::{DateTime, Utc};
use fancy_log::{LogLevel, log};
use ip_lookup::get_public_ip_addr;
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::PathBuf;
use tokio::task;
use uuid::Uuid;

// Represents the structure of our instance.json file.
#[derive(Serialize, Deserialize, Debug)]
struct InstanceConfig {
	instance_id: String,
	seeds: Vec<String>,
	created_at: DateTime<Utc>,
}

// Generates a 16-character ID from a v4 UUID, matching the frontend logic.
fn generate_instance_id() -> String {
	let full_uuid = Uuid::new_v4().to_string();
	let compact_uuid = full_uuid.replace('-', "");
	compact_uuid[..16].to_string()
}

// Generates a vector of 6 unique v4 UUIDs as strings.
fn generate_seeds() -> Vec<String> {
	(0..6).map(|_| Uuid::new_v4().to_string()).collect()
}

/// Gets the base configuration path, consistent with other config logic.
/// It prioritizes the `CONFIG_DIR` env var and defaults to `~/vane`.
fn get_base_config_path() -> PathBuf {
	let config_dir = env::var("CONFIG_DIR").unwrap_or_else(|_| "~/vane".to_string());
	PathBuf::from(shellexpand::tilde(&config_dir).to_string())
}

/// Initializes the instance configuration file (`instance.json`).
/// This is now an async function to handle IP lookup without blocking.
pub async fn initialize_instance_config() -> std::io::Result<()> {
	let base_path = get_base_config_path();
	let instance_file_path = base_path.join("instance.json");

	if instance_file_path.exists() {
		return Ok(());
	}

	log(
		LogLevel::Info,
		"First launch detected. Generating new instance configuration...",
	);

	// This is a blocking network call, so run it in a dedicated thread.
	let public_ip = task::spawn_blocking(|| get_public_ip_addr())
		.await
		.unwrap_or(None) // Handle potential panic from spawn_blocking
		.unwrap_or_else(|| {
			log(
				LogLevel::Warn,
				"Could not determine public IP. Falling back to 127.0.0.1.",
			);
			"127.0.0.1".to_string()
		});

	let port = env::var("PORT").unwrap_or_else(|_| "3333".to_string());
	let base_url = format!("http://{}:{}", public_ip, port);

	let new_config = InstanceConfig {
		instance_id: generate_instance_id(),
		seeds: generate_seeds(),
		created_at: Utc::now(),
	};

	let config_json =
		serde_json::to_string_pretty(&new_config).expect("Failed to serialize instance config");
	fs::write(&instance_file_path, config_json)?;

	print_setup_url(&new_config, &base_url);

	Ok(())
}

/// Constructs and prints a clickable terminal hyperlink containing the base_url, os, and timestamp.
fn print_setup_url(config: &InstanceConfig, base_url: &str) {
	// Fallback to http as requested for initial setup.
	let public_site_url =
		env::var("PUBLIC_SITE_URL").unwrap_or_else(|_| "http://dash.vaneproxy.com".to_string());

	let seeds_payload = config.seeds.join(";");
	let os_info = std::env::consts::OS;
	// Use RFC3339 for a standard, easily parsable timestamp format.
	let timestamp = Utc::now().to_rfc3339();

	// The new payload format: {base_url};{os};{timestamp};{seed1};{seed2};...
	let full_payload = format!("{};{};{};{}", base_url, os_info, timestamp, seeds_payload);
	let encoded_payload = B64.encode(full_payload);

	let setup_url = format!(
		"{}/instance-setup/{}#{}",
		public_site_url, config.instance_id, encoded_payload
	);

	let link_text = "Click here to complete the setup process";
	let hyperlink = format!("\x1B]8;;{}\x07{}\x1B]8;;\x07", setup_url, link_text);

	// Print with the new requested format.
	println!();
	println!("    To complete setup, please open the following link in your browser.");
	println!("    Warning: This link contains sensitive credentials. Do not share it.");
	println!();
	println!("    {}", hyperlink);
	println!();
}
