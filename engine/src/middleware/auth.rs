/* engine/src/middleware/auth.rs */

use crate::common::response;
use axum::{
	body::Body,
	http::{Request, StatusCode},
	middleware::Next,
	response::{IntoResponse, Response},
};
use once_cell::sync::Lazy;
use serde::Deserialize;
use std::{env, fs, path::PathBuf};
use totp::{current_unix_time, verify_combined_token};

#[derive(Deserialize)]
struct InstanceSeeds {
	seeds: [String; 6],
}

static SEEDS: Lazy<[String; 6]> = Lazy::new(|| {
	let config_dir_str = env::var("CONFIG_DIR").unwrap_or_else(|_| "~/vane".to_string());
	let expanded_path = shellexpand::tilde(&config_dir_str).to_string();
	let instance_file_path = PathBuf::from(expanded_path).join("instance.json");
	let file_content = fs::read_to_string(instance_file_path)
		.expect("FATAL: Could not read instance.json for authentication.");
	let parsed: InstanceSeeds =
		serde_json::from_str(&file_content).expect("FATAL: Could not parse seeds from instance.json.");
	parsed.seeds
});

pub async fn auth_middleware(req: Request<Body>, next: Next) -> Response {
	// Skip auth if SKIP_AUTH=true
	let skip_auth = env::var("SKIP_AUTH").unwrap_or_else(|_| "false".to_string()) == "true";
	if skip_auth {
		return next.run(req).await;
	}

	// Bypass root path
	if req.uri().path() == "/" {
		return next.run(req).await;
	}

	let auth_header = req
		.headers()
		.get("Authorization")
		.and_then(|h| h.to_str().ok());

	match auth_header {
		Some(header) if header.starts_with("Bearer ") => {
			let token = &header[7..];
			let seed_slices: [&str; 6] = [
				&SEEDS[0], &SEEDS[1], &SEEDS[2], &SEEDS[3], &SEEDS[4], &SEEDS[5],
			];

			let is_valid = verify_combined_token(seed_slices, current_unix_time(), token, 30, 2, "s");

			if is_valid {
				next.run(req).await
			} else {
				response::error(
					StatusCode::FORBIDDEN,
					"Invalid authentication token.".to_string(),
				)
				.into_response()
			}
		}
		_ => response::error(
			StatusCode::UNAUTHORIZED,
			"Missing or invalid authorization header.".to_string(),
		)
		.into_response(),
	}
}
