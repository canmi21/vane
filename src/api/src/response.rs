/* src/api/src/response.rs */

use axum::{
	http::StatusCode,
	response::{IntoResponse, Json, Response},
};
use serde::Serialize;

// A generic structure for all API responses (Runtime only, no ToSchema here).
#[derive(Serialize)]
pub struct ApiResponse<T: Serialize> {
	pub status: &'static str,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub data: Option<T>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub message: Option<String>,
}

/// Creates a successful (200 OK) API response.
pub fn success<T: Serialize>(data: T) -> Response {
	let response = ApiResponse { status: "success", data: Some(data), message: None };
	(StatusCode::OK, Json(response)).into_response()
}

/// Creates a created (201 Created) API response.
pub fn created<T: Serialize>(data: T) -> Response {
	let response = ApiResponse { status: "success", data: Some(data), message: None };
	(StatusCode::CREATED, Json(response)).into_response()
}

/// Creates an error API response.
#[must_use]
pub fn error(status_code: StatusCode, message: String) -> Response {
	let response = ApiResponse::<()> { status: "error", data: None, message: Some(message) };
	(status_code, Json(response)).into_response()
}
