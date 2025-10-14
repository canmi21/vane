/* engine/src/modules/instance.rs */

use crate::common::response;
use axum::{http::StatusCode, response::IntoResponse};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{env, fs, path::PathBuf};

// A struct to represent the public-facing instance information.
#[derive(Serialize)]
struct InstanceInfo {
	instance_id: String,
	created_at: DateTime<Utc>,
}

// A struct to parse the full instance.json file.
#[derive(Deserialize)]
struct InstanceFile {
	instance_id: String,
	created_at: DateTime<Utc>,
}

/// Handler for the GET /v1/instance endpoint.
/// It reads the instance configuration file and returns public information.
pub async fn get_instance_info() -> impl IntoResponse {
	let config_dir_str = env::var("CONFIG_DIR").unwrap_or_else(|_| "~/vane".to_string());
	let expanded_path = shellexpand::tilde(&config_dir_str).to_string();
	let instance_file_path = PathBuf::from(expanded_path).join("instance.json");

	// FIX 3: Instead of matching directly and returning, we match and assign to a variable.
	// This allows Rust to unify the return type of the match expression.
	let result = match fs::read_to_string(instance_file_path) {
		Ok(content) => match serde_json::from_str::<InstanceFile>(&content) {
			Ok(data) => {
				let info = InstanceInfo {
					instance_id: data.instance_id,
					created_at: data.created_at,
				};
				// Both branches now return a Result that can be converted to a response.
				Ok(response::success(info))
			}
			Err(_) => Err(response::error(
				StatusCode::INTERNAL_SERVER_ERROR,
				"Failed to parse instance configuration.".to_string(),
			)),
		},
		Err(_) => Err(response::error(
			StatusCode::INTERNAL_SERVER_ERROR,
			"Instance configuration not found.".to_string(),
		)),
	};

	// Convert the final result into a response.
	match result {
		Ok(res) => res.into_response(),
		Err(res) => res.into_response(),
	}
}
