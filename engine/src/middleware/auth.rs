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

// A struct to parse only the seeds from instance.json.
#[derive(Deserialize)]
struct InstanceSeeds {
	seeds: [String; 6],
}

// Use `once_cell::Lazy` to read the seeds from the file only ONCE at startup.
// This is crucial for performance. If the file is missing or invalid, the server will panic on
// the first authenticated request, which is a desired fail-fast behavior.
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

/// Axum middleware for TOTP-based authentication.
// FIX 1: Changed the return type to a single `Response`.
// We will now return `impl IntoResponse` and convert it at the end.
pub async fn auth_middleware(req: Request<Body>, next: Next) -> Response {
	// Bypass authentication for the root path.
	if req.uri().path() == "/" {
		return next.run(req).await;
	}

	// Attempt to extract the "Authorization" header.
	let auth_header = req
		.headers()
		.get("Authorization")
		.and_then(|h| h.to_str().ok());

	match auth_header {
		Some(header) if header.starts_with("Bearer ") => {
			let token = &header[7..]; // Strip "Bearer " prefix

			// Convert seeds from Vec<String> to [&str; 6] for the verification function.
			let seed_slices: [&str; 6] = [
				&SEEDS[0], &SEEDS[1], &SEEDS[2], &SEEDS[3], &SEEDS[4], &SEEDS[5],
			];

			// Verify the token. Allow a tolerance of +/- 1 window (30s).
			// A value of `2` for `allowed_windows` in your crate checks `[-1, 0, 1]`.
			let is_valid = verify_combined_token(
				seed_slices,
				current_unix_time(),
				token,
				30, // 30-second window
				2,  // allows for `t-1`, `t`, `t+1`
				"s",
			);

			if is_valid {
				// If valid, proceed to the handler.
				next.run(req).await
			} else {
				// If token is present but invalid.
				// FIX 2: Call `.into_response()` to convert the error into a `Response`.
				response::error(
					StatusCode::FORBIDDEN,
					"Invalid authentication token.".to_string(),
				)
				.into_response()
			}
		}
		_ => {
			// If the header is missing or malformed.
			// FIX 2: Call `.into_response()` to convert the error into a `Response`.
			response::error(
				StatusCode::UNAUTHORIZED,
				"Missing or invalid authorization header.".to_string(),
			)
			.into_response()
		}
	}
}
