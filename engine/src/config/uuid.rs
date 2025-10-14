/* engine/src/config/uuid.rs */

use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use chrono::{DateTime, Utc};
use fancy_log::{LogLevel, log};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::PathBuf;
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
///
/// This function checks if `instance.json` exists. If not, it generates a new
/// instance ID and seeds, saves them, and logs a one-time setup URL for the user.
/// It assumes the base configuration directory has already been created.
pub fn initialize_instance_config() -> std::io::Result<()> {
	let base_path = get_base_config_path();
	let instance_file_path = base_path.join("instance.json");

	// If the config file already exists, our work here is done.
	if instance_file_path.exists() {
		return Ok(());
	}

	log(
		LogLevel::Info,
		"First launch detected. Generating new instance configuration...",
	);

	// Generate new instance data.
	let new_config = InstanceConfig {
		instance_id: generate_instance_id(),
		seeds: generate_seeds(),
		created_at: Utc::now(),
	};

	// Write the new configuration to instance.json.
	let config_json =
		serde_json::to_string_pretty(&new_config).expect("Failed to serialize instance config");
	fs::write(&instance_file_path, config_json)?;

	// The log for successful creation has been removed as requested.

	// Print the setup URL directly to the console for better visibility.
	print_setup_url(&new_config);

	Ok(())
}

/// Constructs a clickable terminal hyperlink and prints it to the console without revealing the raw URL.
fn print_setup_url(config: &InstanceConfig) {
	// Read the public site URL from .env, with a fallback.
	let public_site_url =
		env::var("PUBLIC_SITE_URL").unwrap_or_else(|_| "https://dash.vaneproxy.com".to_string());

	// Create the semicolon-separated payload string from seeds.
	let seeds_payload = config.seeds.join(";");

	// Base64 encode the payload. The fragment part (#) of the URL ensures it's not sent to the server.
	let encoded_seeds = B64.encode(seeds_payload);

	let setup_url = format!(
		"{}/instance-setup/{}#{}",
		public_site_url, config.instance_id, encoded_seeds
	);

	// Create a clickable hyperlink using ANSI escape codes (OSC 8).
	// The raw URL is hidden within the escape sequence and is not printed directly.
	let link_text = "(Click here) to complete the setup process";
	let hyperlink = format!("\x1B]8;;{}\x07{}\x1B]8;;\x07", setup_url, link_text);

	// Print the message directly to the console for maximum visibility.
	println!(); // An empty line for spacing.
	println!("    To complete setup, please open the following link in your browser.");
	println!("    Warning: This link contains sensitive credentials. Do not share it.");
	println!();
	println!("    {}", hyperlink);
	println!();
}
