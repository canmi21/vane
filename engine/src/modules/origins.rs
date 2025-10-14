/* engine/src/modules/origins.rs */

use crate::common::response;
use axum::{
	Json,
	extract::Path,
	http::StatusCode,
	response::{IntoResponse, Response},
};
use once_cell::sync::Lazy;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, env, fs, net::IpAddr, path::PathBuf, str::FromStr, sync::Arc};
use tokio::sync::RwLock;
use url::Url;

// --- Data Structures ---
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Origin {
	pub scheme: String,
	pub host: String,
	pub port: u16,
	pub path: String,
	#[serde(default)]
	pub skip_ssl_verify: bool,
	pub raw_url: String,
}

// A new struct for API responses that includes the ID.
#[derive(Serialize)]
pub struct OriginResponse {
	pub id: String,
	#[serde(flatten)] // Merges the fields of `Origin` into this struct
	pub origin: Origin,
}

type OriginsStore = HashMap<String, Origin>;

// --- State Management ---
static ORIGINS: Lazy<Arc<RwLock<OriginsStore>>> = Lazy::new(|| {
	let path = get_origins_path();
	if let Some(parent) = path.parent() {
		if !parent.exists() {
			fs::create_dir_all(parent).expect("Failed to create config directory for origins.json");
		}
	}
	let origins = match fs::read_to_string(path) {
		Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
		Err(_) => OriginsStore::new(),
	};
	Arc::new(RwLock::new(origins))
});

fn get_origins_path() -> PathBuf {
	let config_dir = env::var("CONFIG_DIR").unwrap_or_else(|_| "~/vane".to_string());
	let expanded_path = shellexpand::tilde(&config_dir).to_string();
	PathBuf::from(expanded_path).join("origins.json")
}

async fn save_origins(data_to_save: &OriginsStore) -> Result<(), std::io::Error> {
	let path = get_origins_path();
	let contents = serde_json::to_string_pretty(data_to_save).unwrap();
	tokio::fs::write(path, contents).await
}

fn generate_unique_id(existing_ids: &OriginsStore) -> String {
	const CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
	let mut rng = rand::rng();
	loop {
		let id: String = (0..5)
			.map(|_| {
				let idx = rng.random_range(0..CHARSET.len());
				CHARSET[idx] as char
			})
			.collect();

		if !existing_ids.contains_key(&id) {
			return id;
		}
	}
}

// --- API Payloads ---
#[derive(Deserialize)]
pub struct CreateOriginPayload {
	pub url: String,
}

// New payload for partial updates. All fields are optional.
#[derive(Deserialize, Debug)]
pub struct UpdateOriginPayload {
	pub raw_url: Option<String>,
	pub scheme: Option<String>,
	pub host: Option<String>,
	pub port: Option<u16>,
	pub path: Option<String>,
	pub skip_ssl_verify: Option<bool>,
}

// --- Axum Handlers ---
/// GET /v1/origins - List all configured origins.
pub async fn list_origins() -> impl IntoResponse {
	let origins = ORIGINS.read().await;
	let origins_vec: Vec<OriginResponse> = origins
		.iter()
		.map(|(id, origin)| OriginResponse {
			id: id.clone(),
			origin: origin.clone(),
		})
		.collect();
	response::success(origins_vec)
}

/// POST /v1/origins - Create a new origin.
pub async fn create_origin(Json(payload): Json<CreateOriginPayload>) -> Response {
	match parse_and_validate_origin_url(&payload.url) {
		Ok(parsed) => {
			let mut origins = ORIGINS.write().await;
			let new_id = generate_unique_id(&origins);
			let new_origin = Origin {
				scheme: parsed.scheme,
				host: parsed.host,
				port: parsed.port,
				path: parsed.path,
				skip_ssl_verify: false,
				raw_url: payload.url,
			};
			origins.insert(new_id.clone(), new_origin.clone());

			if save_origins(&origins).await.is_err() {
				return response::error(
					StatusCode::INTERNAL_SERVER_ERROR,
					"Failed to save origin.".to_string(),
				)
				.into_response();
			}

			let response_data = OriginResponse {
				id: new_id,
				origin: new_origin,
			};
			(StatusCode::CREATED, Json(response_data)).into_response()
		}
		Err(response) => response,
	}
}

/// GET /v1/origins/{id} - Get a single origin by its ID.
pub async fn get_origin(Path(id): Path<String>) -> Response {
	let origins = ORIGINS.read().await;
	match origins.get(&id) {
		Some(origin) => {
			let response_data = OriginResponse {
				id,
				origin: origin.clone(),
			};
			response::success(response_data).into_response()
		}
		None => response::error(StatusCode::NOT_FOUND, "Origin not found.".to_string()).into_response(),
	}
}

/// PUT /v1/origins/{id} - Partially update an existing origin.
pub async fn update_origin(
	Path(id): Path<String>,
	Json(payload): Json<UpdateOriginPayload>,
) -> Response {
	let mut origins = ORIGINS.write().await;
	let existing_origin = match origins.get_mut(&id) {
		Some(origin) => origin,
		None => {
			return response::error(StatusCode::NOT_FOUND, "Origin not found.".to_string())
				.into_response();
		}
	};

	// If a new `raw_url` is provided, re-parse it fully.
	if let Some(raw_url) = payload.raw_url {
		match parse_and_validate_origin_url(&raw_url) {
			Ok(parsed) => {
				existing_origin.scheme = parsed.scheme;
				existing_origin.host = parsed.host;
				existing_origin.port = parsed.port;
				existing_origin.path = parsed.path;
				existing_origin.raw_url = raw_url;
			}
			Err(response) => return response,
		}
	} else {
		// Otherwise, apply individual field updates if they exist.
		if let Some(scheme) = payload.scheme {
			existing_origin.scheme = scheme;
		}
		if let Some(host) = payload.host {
			existing_origin.host = host;
		}
		if let Some(port) = payload.port {
			existing_origin.port = port;
		}
		if let Some(path) = payload.path {
			existing_origin.path = path;
		}
	}

	// `skip_ssl_verify` can always be updated independently.
	if let Some(skip) = payload.skip_ssl_verify {
		existing_origin.skip_ssl_verify = skip;
	}

	let response_data = OriginResponse {
		id,
		origin: existing_origin.clone(),
	};

	if save_origins(&origins).await.is_err() {
		return response::error(
			StatusCode::INTERNAL_SERVER_ERROR,
			"Failed to save origin.".to_string(),
		)
		.into_response();
	}

	response::success(response_data).into_response()
}

/// DELETE /v1/origins/{id} - Delete an origin by its ID.
pub async fn delete_origin(Path(id): Path<String>) -> Response {
	let mut origins = ORIGINS.write().await;
	if origins.remove(&id).is_some() {
		if save_origins(&origins).await.is_err() {
			return response::error(
				StatusCode::INTERNAL_SERVER_ERROR,
				"Failed to save changes after deleting origin.".to_string(),
			)
			.into_response();
		}
		StatusCode::NO_CONTENT.into_response()
	} else {
		response::error(StatusCode::NOT_FOUND, "Origin not found.".to_string()).into_response()
	}
}

// --- Helper Logic ---
struct ParsedOrigin {
	scheme: String,
	host: String,
	port: u16,
	path: String,
}

fn parse_and_validate_origin_url(raw_url: &str) -> Result<ParsedOrigin, Response> {
	let trimmed_url = raw_url.trim();
	if trimmed_url.is_empty() {
		return Err(
			response::error(StatusCode::BAD_REQUEST, "URL cannot be empty.".to_string()).into_response(),
		);
	}

	let url = match Url::parse(trimmed_url) {
		Ok(url) => url,
		Err(_) if !trimmed_url.contains("://") => {
			match Url::parse(&format!("dummy://{}", trimmed_url)) {
				Ok(url) => url,
				Err(_) => {
					return Err(
						response::error(StatusCode::BAD_REQUEST, "Invalid URL format.".to_string())
							.into_response(),
					);
				}
			}
		}
		Err(_) => {
			return Err(
				response::error(StatusCode::BAD_REQUEST, "Invalid URL format.".to_string()).into_response(),
			);
		}
	};

	let host = match url.host_str() {
		Some(h) => h.to_string(),
		None => {
			return Err(
				response::error(StatusCode::BAD_REQUEST, "URL must have a host.".to_string())
					.into_response(),
			);
		}
	};

	let (scheme, port) = match url.scheme() {
		"http" => ("http".to_string(), url.port().unwrap_or(80)),
		"https" => ("https".to_string(), url.port().unwrap_or(443)),
		"dummy" => {
			if IpAddr::from_str(&host).is_ok() {
				("http".to_string(), url.port().unwrap_or(80))
			} else {
				("https".to_string(), url.port().unwrap_or(443))
			}
		}
		scheme => {
			return Err(
				response::error(
					StatusCode::BAD_REQUEST,
					format!("Unsupported scheme: {}", scheme),
				)
				.into_response(),
			);
		}
	};

	let path = if url.path().is_empty() || (url.path() == "/" && !trimmed_url.ends_with('/')) {
		"/"
	} else {
		url.path()
	}
	.to_string();

	Ok(ParsedOrigin {
		scheme,
		host,
		port,
		path,
	})
}
