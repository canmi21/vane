/* src/api/handlers/root.rs */

use crate::api::response;
use axum::response::IntoResponse;
use chrono::Utc;
use serde_json::{Map, Value, json};
use std::env;

pub async fn root_handler() -> impl IntoResponse {
	// --- Package Info (from Cargo.toml) ---
	let pkg_name_raw = env!("CARGO_PKG_NAME");
	let pkg_version = env!("CARGO_PKG_VERSION");
	let repository = env!("CARGO_PKG_REPOSITORY");
	let license = env!("CARGO_PKG_LICENSE");

	// Auto-get and format the package name for the "version" field.
	let pkg_name_formatted = {
		let lower = pkg_name_raw.to_lowercase();
		let mut chars = lower.chars();
		match chars.next() {
			None => String::new(),
			Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
		}
	};

	// --- Build Info (from build.rs) ---
	let git_commit = env!("GIT_COMMIT_SHORT");
	let rustc_version = env!("RUSTC_FULL_VERSION");
	let cargo_version = env!("CARGO_FULL_VERSION");
	let build_date = env!("BUILD_DATE");

	// --- Platform & Dynamic Info (runtime) ---
	let arch = env::consts::ARCH;
	let os = env::consts::OS;
	let request_timestamp = Utc::now().to_rfc3339();

	// The string that mimics the `--version` output. This is the VALUE.
	let version_string = format!("{pkg_name_raw} {pkg_version} ({git_commit} {build_date})");

	// Create the "build" object manually to support a dynamic key.
	let mut build_map = Map::new();
	// The KEY is the dynamic, lowercased package name.
	build_map.insert(pkg_name_raw.to_lowercase(), Value::String(version_string));
	build_map.insert("rust".to_owned(), Value::String(rustc_version.into()));
	build_map.insert("cargo".to_owned(), Value::String(cargo_version.into()));

	response::success(json!({
			"package": {
					"author": "Canmi(Canmi21) t@canmi.icu",
					"version": format!("{} v{}", pkg_name_formatted, pkg_version),
					"license": license,
					"repository": repository,
			},
			"build": build_map, // Use the dynamically created map here.
			"runtime": {
					"arch": arch,
					"platform": os,
			},
			"timestamp": request_timestamp,
	}))
}
