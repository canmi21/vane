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

#[cfg(test)]
mod tests {
	use super::*;
	use axum::body::to_bytes;
	use axum::response::Response;
	use serde_json::json;
	use std::env;
	use std::fs;
	use tempfile::tempdir;

	#[tokio::test]
	async fn test_get_instance_info_success() {
		let dir = tempdir().unwrap();
		let config_path = dir.path().join("instance.json");

		let instance_data = json!({
			"instance_id": "test-id-123",
			"created_at": "2025-10-14T12:00:00Z"
		});
		fs::write(
			&config_path,
			serde_json::to_string_pretty(&instance_data).unwrap(),
		)
		.unwrap();

		let original = env::var("CONFIG_DIR").ok();
		unsafe { env::set_var("CONFIG_DIR", dir.path()) };

		let resp: Response = get_instance_info().await.into_response();
		assert_eq!(resp.status(), axum::http::StatusCode::OK);

		let body_bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
		let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();
		assert!(body_str.contains("test-id-123"));

		if let Some(orig) = original {
			unsafe { env::set_var("CONFIG_DIR", orig) };
		} else {
			unsafe { env::remove_var("CONFIG_DIR") };
		}
	}

	#[tokio::test]
	async fn test_get_instance_info_missing_file() {
		let dir = tempdir().unwrap();

		let original = env::var("CONFIG_DIR").ok();
		unsafe { env::set_var("CONFIG_DIR", dir.path()) };

		let resp: Response = get_instance_info().await.into_response();
		assert_eq!(resp.status(), axum::http::StatusCode::INTERNAL_SERVER_ERROR);

		if let Some(orig) = original {
			unsafe { env::set_var("CONFIG_DIR", orig) };
		} else {
			unsafe { env::remove_var("CONFIG_DIR") };
		}
	}

	#[tokio::test]
	async fn test_get_instance_info_invalid_json() {
		let dir = tempdir().unwrap();
		let config_path = dir.path().join("instance.json");

		fs::write(&config_path, "not a json").unwrap();

		let original = env::var("CONFIG_DIR").ok();
		unsafe { env::set_var("CONFIG_DIR", dir.path()) };

		let resp: Response = get_instance_info().await.into_response();
		assert_eq!(resp.status(), axum::http::StatusCode::INTERNAL_SERVER_ERROR);

		if let Some(orig) = original {
			unsafe { env::set_var("CONFIG_DIR", orig) };
		} else {
			unsafe { env::remove_var("CONFIG_DIR") };
		}
	}
}
