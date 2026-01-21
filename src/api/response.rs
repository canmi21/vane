/* src/api/response.rs */

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
#[must_use]
pub fn error(status_code: StatusCode, message: String) -> impl IntoResponse {
	let response = ApiResponse::<()> {
		// No data is sent on error
		status: "error",
		data: None,
		message: Some(message),
	};
	(status_code, Json(response))
}

#[cfg(test)]
mod tests {
	use super::*;
	use axum::body::to_bytes;
	use axum::response::Response;
	use serde::Serialize;
	use serde_json::{Value, json};

	/// A simple struct for testing success responses with data.
	#[derive(Serialize, Debug, PartialEq)]
	struct TestData {
		id: u32,
		name: String,
	}

	/// Helper function to get the JSON body from a response.
	async fn get_body_as_json(response: Response) -> Value {
		let body_bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
		serde_json::from_slice(&body_bytes).unwrap()
	}

	/// Tests that the success response is formatted correctly.
	#[tokio::test]
	async fn test_success_response_format() {
		let test_data = TestData {
			id: 123,
			name: "test-item".to_string(),
		};

		let response = success(test_data).into_response();

		assert_eq!(response.status(), StatusCode::OK);

		let body = get_body_as_json(response).await;

		assert_eq!(body["status"], "success");
		assert_eq!(body["data"]["id"], 123);
		assert_eq!(body["data"]["name"], "test-item");
		assert_eq!(body["message"], json!(null));
	}

	/// Tests that the error response is formatted correctly.
	#[tokio::test]
	async fn test_error_response_format() {
		let error_message = "Resource not found".to_string();
		let status_code = StatusCode::NOT_FOUND;

		let response = error(status_code, error_message.clone()).into_response();

		assert_eq!(response.status(), status_code);

		let body = get_body_as_json(response).await;

		assert_eq!(body["status"], "error");
		assert_eq!(body["message"], error_message);
		assert_eq!(body["data"], json!(null));
	}
}
