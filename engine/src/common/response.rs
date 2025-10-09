/* engine/src/common/response.rs */

use axum::{
	http::StatusCode,
	response::{IntoResponse, Json},
};
use serde::Serialize;

// A generic structure for all API responses.
#[derive(Serialize)]
struct ApiResponse<T: Serialize> {
	status: &'static str,
	#[serde(skip_serializing_if = "Option::is_none")]
	data: Option<T>,
	#[serde(skip_serializing_if = "Option::is_none")]
	message: Option<String>,
}

/// Creates a successful (200 OK) API response.
///
/// # Arguments
///
/// * `data` - Any data that implements `serde::Serialize`.
///
pub fn success<T: Serialize>(data: T) -> impl IntoResponse {
	let response = ApiResponse {
		status: "success",
		data: Some(data),
		message: None,
	};
	(StatusCode::OK, Json(response))
}

/// Creates an error API response.
///
/// # Arguments
///
/// * `status_code` - The HTTP status code for the response.
/// * `message` - A descriptive error message.
///
pub fn error(status_code: StatusCode, message: String) -> impl IntoResponse {
	let response = ApiResponse::<()> {
		// No data is sent on error
		status: "error",
		data: None,
		message: Some(message),
	};
	(status_code, Json(response))
}
