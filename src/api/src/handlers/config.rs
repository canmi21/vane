/* src/api/handlers/config.rs */

use crate::response;
use crate::schemas::config::{
	ImportResponse, ImportResult, ReloadRequest, ReloadResponse, ReloadResult,
};
use axum::{
	Json,
	body::Body,
	extract::Multipart,
	http::{HeaderMap, HeaderValue, StatusCode},
	response::IntoResponse,
};
use chrono::Utc;
use fancy_log::{LogLevel, log};
use std::collections::HashMap;
use std::io::Read;
use tokio::fs;
use vane_primitives::common::config::file_loader;

// --- Helpers ---

fn clean_existing_config<'a>(
	dir: &'a std::path::Path,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = std::io::Result<()>> + Send + 'a>> {
	Box::pin(async move {
		if !dir.exists() {
			return Ok(());
		}
		let Ok(mut entries) = fs::read_dir(dir).await else {
			return Ok(());
		};

		while let Ok(Some(entry)) = entries.next_entry().await {
			let path = entry.path();
			if path.is_dir() {
				clean_existing_config(&path).await?;
			} else if let Some(ext) = path.extension().and_then(|s| s.to_str())
				&& matches!(ext, "json" | "yaml" | "yml" | "toml")
			{
				let _ = fs::remove_file(path).await;
			}
		}
		Ok(())
	})
}

// --- Handlers ---

/// Reload configuration
#[utoipa::path(
    post,
    path = "/config/reload",
    request_body = Option<ReloadRequest>,
    responses((
        status = 200,
        description = "Config reloaded",
        body = ReloadResponse
    )),
    tag = "config",
    security(("bearer_auth" = []))
)]
pub async fn reload_config_handler(Json(req): Json<Option<ReloadRequest>>) -> impl IntoResponse {
	let config_dir = file_loader::get_config_dir();
	let reload_marker = config_dir.join(".reload");

	let timestamp = Utc::now().to_rfc3339();
	let _ = fs::write(&reload_marker, &timestamp).await;

	response::success(ReloadResult {
		reloaded: req.map(|r| r.components.unwrap_or_default()).unwrap_or_else(|| vec!["all".into()]),
		timestamp,
	})
}

/// Export configuration
#[utoipa::path(
    get,
    path = "/config/export",
    responses((
        status = 200,
        description = "Configuration archive",
        content_type = "application/gzip"
    )),
    tag = "config",
    security(("bearer_auth" = []))
)]
pub async fn export_config_handler() -> impl IntoResponse {
	let config_dir = file_loader::get_config_dir();

	let mut tar_builder = tar::Builder::new(Vec::new());

	if let Err(e) = tar_builder.append_dir_all(".", &config_dir) {
		log(LogLevel::Error, &format!("Failed to build config archive: {e}"));
		return response::error(
			StatusCode::INTERNAL_SERVER_ERROR,
			format!("Failed to build archive: {e}"),
		);
	}

	let tar_data = match tar_builder.into_inner() {
		Ok(d) => d,
		Err(e) => {
			log(LogLevel::Error, &format!("Failed to finish config archive: {e}"));
			return response::error(
				StatusCode::INTERNAL_SERVER_ERROR,
				format!("Failed to finish archive: {e}"),
			);
		}
	};

	let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
	if let Err(e) = std::io::Write::write_all(&mut encoder, &tar_data) {
		log(LogLevel::Error, &format!("Config compression failed: {e}"));
		return response::error(StatusCode::INTERNAL_SERVER_ERROR, format!("Compression failed: {e}"));
	}

	let compressed_data = match encoder.finish() {
		Ok(d) => d,
		Err(e) => {
			log(LogLevel::Error, &format!("Config compression finish failed: {e}"));
			return response::error(
				StatusCode::INTERNAL_SERVER_ERROR,
				format!("Compression finish failed: {e}"),
			);
		}
	};

	let filename = format!("vane-config-{}.tar.gz", Utc::now().format("%Y%m%d-%H%M%S"));

	let mut headers = HeaderMap::new();
	headers.insert(axum::http::header::CONTENT_TYPE, HeaderValue::from_static("application/gzip"));
	headers.insert(
		axum::http::header::CONTENT_DISPOSITION,
		HeaderValue::from_str(&format!("attachment; filename=\"{filename}\"",)).unwrap(),
	);

	(headers, Body::from(compressed_data)).into_response()
}

/// Import configuration
#[utoipa::path(
    post,
    path = "/config/import",
    responses((
        status = 200,
        description = "Config imported",
        body = ImportResponse
    ), (
        status = 400,
        description = "Invalid file or multipart request"
    )),
    tag = "config",
    security(("bearer_auth" = []))
)]
pub async fn import_config_handler(mut multipart: Multipart) -> impl IntoResponse {
	let mut file_data = None;

	while let Ok(Some(field)) = multipart.next_field().await {
		let name = field.name().unwrap_or_default().to_owned();
		if name == "file" {
			match field.bytes().await {
				Ok(bytes) => {
					file_data = Some(bytes);
					break;
				}
				Err(e) => {
					log(LogLevel::Error, &format!("Failed to read uploaded file bytes: {e}"));
					return response::error(StatusCode::BAD_REQUEST, format!("Failed to read file: {e}"));
				}
			}
		}
	}

	let Some(data) = file_data else {
		return response::error(StatusCode::BAD_REQUEST, "Missing 'file' field".into());
	};

	// 1. Decompress Gzip
	let mut decoder = flate2::read::GzDecoder::new(&data[..]);
	let mut tar_data = Vec::new();
	if let Err(e) = decoder.read_to_end(&mut tar_data) {
		log(LogLevel::Error, &format!("Gzip decompression failed during import: {e}"));
		return response::error(StatusCode::BAD_REQUEST, format!("Failed to decompress Gzip: {e}"));
	}

	// 2. Clean existing config (Restore mode)
	let config_dir = file_loader::get_config_dir();
	if let Err(e) = clean_existing_config(&config_dir).await {
		log(LogLevel::Error, &format!("Failed to clean existing config: {e}"));
		return response::error(
			StatusCode::INTERNAL_SERVER_ERROR,
			format!("Failed to clean existing config: {e}"),
		);
	}

	// 3. Unpack Tar and Count
	let mut archive = tar::Archive::new(&tar_data[..]);
	let mut file_count = 0;

	let entries = match archive.entries() {
		Ok(entries) => entries,
		Err(e) => {
			log(LogLevel::Error, &format!("Failed to read tar entries: {e}"));
			return response::error(StatusCode::BAD_REQUEST, format!("Invalid tar archive: {e}"));
		}
	};

	for entry in entries {
		let mut entry = match entry {
			Ok(e) => e,
			Err(e) => {
				log(LogLevel::Warn, &format!("Skipping corrupt tar entry: {e}"));
				continue;
			}
		};

		if let Err(e) = entry.unpack_in(&config_dir) {
			log(LogLevel::Error, &format!("Failed to unpack file from archive: {e}"));
			// We continue unpacking other files, but log the error.
			// Alternatively, we could return early. For bulk import, continuing is often better,
			// but for config integrity, maybe failing is safer?
			// Let's stop on critical error to prevent partial corrupt state.
			return response::error(
				StatusCode::INTERNAL_SERVER_ERROR,
				format!("Failed to unpack file: {e}"),
			);
		}
		file_count += 1;
	}

	// 4. Trigger reload
	let reload_marker = config_dir.join(".reload");
	let _ = fs::write(&reload_marker, Utc::now().to_rfc3339()).await;

	log(LogLevel::Info, &format!("Config imported successfully. {file_count} files restored.",));

	response::success(ImportResult { imported: HashMap::from([("config_files".into(), file_count)]) })
}
