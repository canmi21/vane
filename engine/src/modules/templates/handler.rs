/* engine/src/modules/templates/handler.rs */

use crate::{common::response, daemon::config};
use axum::{
	Json,
	extract::Path,
	http::StatusCode,
	response::{IntoResponse, Response},
};
use fancy_log::{LogLevel, log};
use serde::{Deserialize, Serialize};

// --- API Payloads ---

#[derive(Serialize)]
pub struct ListTemplatesResponse {
	pub templates: Vec<String>,
}

#[derive(Deserialize, Serialize)]
pub struct TemplatePayload {
	pub html_base64: String,
}

// --- Axum Handlers ---

/// Lists all `.html` files in the top level of the templates directory.
pub async fn list_templates() -> Response {
	log(LogLevel::Debug, "GET /v1/templates called");
	let templates_dir = config::get_templates_dir();
	let mut templates = Vec::new();

	if !templates_dir.exists() {
		return response::success(ListTemplatesResponse { templates }).into_response();
	}

	let mut entries = match tokio::fs::read_dir(templates_dir).await {
		Ok(e) => e,
		Err(_) => {
			return response::error(
				StatusCode::INTERNAL_SERVER_ERROR,
				"Failed to read templates directory".to_string(),
			)
			.into_response();
		}
	};

	while let Ok(Some(entry)) = entries.next_entry().await {
		let path = entry.path();
		if path.is_file() {
			if let Some(filename) = path.file_name().and_then(|s| s.to_str()) {
				if filename.ends_with(".html") {
					templates.push(filename.to_string());
				}
			}
		}
	}

	templates.sort();
	response::success(ListTemplatesResponse { templates }).into_response()
}

/// Retrieves the content of a specific template file, base64 encoded.
pub async fn get_template_content(Path(name): Path<String>) -> Response {
	log(
		LogLevel::Info,
		&format!("GET /v1/templates/{} called", name),
	);

	let mut path = config::get_templates_dir();
	path.push(format!("{}.html", name));

	if !path.exists() {
		return response::error(StatusCode::NOT_FOUND, "Template not found.".to_string())
			.into_response();
	}

	match tokio::fs::read(&path).await {
		Ok(content_bytes) => {
			let html_base64 =
				base64::Engine::encode(&base64::engine::general_purpose::STANDARD, content_bytes);
			response::success(TemplatePayload { html_base64 }).into_response()
		}
		Err(e) => {
			log(
				LogLevel::Error,
				&format!("Failed to read template file '{}': {}", path.display(), e),
			);
			response::error(
				StatusCode::INTERNAL_SERVER_ERROR,
				"Failed to read template file.".to_string(),
			)
			.into_response()
		}
	}
}

/// Creates a new template file.
pub async fn create_template(
	Path(name): Path<String>,
	Json(payload): Json<TemplatePayload>,
) -> Response {
	log(
		LogLevel::Info,
		&format!("POST /v1/templates/{} called", name),
	);

	let html_content = match base64::Engine::decode(
		&base64::engine::general_purpose::STANDARD,
		payload.html_base64,
	) {
		Ok(bytes) => bytes,
		Err(_) => {
			return response::error(
				StatusCode::BAD_REQUEST,
				"Invalid base64 content.".to_string(),
			)
			.into_response();
		}
	};

	let mut path = config::get_templates_dir();
	path.push(format!("{}.html", name));

	if path.exists() {
		return response::error(StatusCode::CONFLICT, "Template already exists.".to_string())
			.into_response();
	}

	if let Err(e) = tokio::fs::write(&path, &html_content).await {
		log(
			LogLevel::Error,
			&format!("Failed to create template file '{}': {}", path.display(), e),
		);
		return response::error(
			StatusCode::INTERNAL_SERVER_ERROR,
			"Failed to create template file.".to_string(),
		)
		.into_response();
	}

	(StatusCode::CREATED, "Template created successfully.").into_response()
}

/// Updates an existing template file.
pub async fn update_template(
	Path(name): Path<String>,
	Json(payload): Json<TemplatePayload>,
) -> Response {
	log(
		LogLevel::Info,
		&format!("PUT /v1/templates/{} called", name),
	);

	let html_content = match base64::Engine::decode(
		&base64::engine::general_purpose::STANDARD,
		payload.html_base64,
	) {
		Ok(bytes) => bytes,
		Err(_) => {
			return response::error(
				StatusCode::BAD_REQUEST,
				"Invalid base64 content.".to_string(),
			)
			.into_response();
		}
	};

	let mut path = config::get_templates_dir();
	path.push(format!("{}.html", name));

	if !path.exists() {
		return response::error(StatusCode::NOT_FOUND, "Template not found.".to_string())
			.into_response();
	}

	if let Err(e) = tokio::fs::write(&path, &html_content).await {
		log(
			LogLevel::Error,
			&format!("Failed to update template file '{}': {}", path.display(), e),
		);
		return response::error(
			StatusCode::INTERNAL_SERVER_ERROR,
			"Failed to update template file.".to_string(),
		)
		.into_response();
	}

	(StatusCode::OK, "Template updated successfully.").into_response()
}

/// Deletes a template file.
pub async fn delete_template(Path(name): Path<String>) -> Response {
	log(
		LogLevel::Info,
		&format!("DELETE /v1/templates/{} called", name),
	);

	let mut path = config::get_templates_dir();
	path.push(format!("{}.html", name));

	if !path.exists() {
		return response::error(StatusCode::NOT_FOUND, "Template not found.".to_string())
			.into_response();
	}

	if let Err(e) = tokio::fs::remove_file(&path).await {
		log(
			LogLevel::Error,
			&format!("Failed to delete template file '{}': {}", path.display(), e),
		);
		return response::error(
			StatusCode::INTERNAL_SERVER_ERROR,
			"Failed to delete template file.".to_string(),
		)
		.into_response();
	}

	StatusCode::NO_CONTENT.into_response()
}
