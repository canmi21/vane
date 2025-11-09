/* src/middleware/logger.rs */

use axum::{
	body::Body,
	http::{Method, Request},
	middleware::Next,
	response::Response,
};
use fancy_log::{LogLevel, log};

/// Axum middleware to log all incoming requests.
///
/// It uses `LogLevel::Info` for mutating methods (POST, DELETE, etc.)
/// and `LogLevel::Debug` for non-mutating methods (GET, HEAD, etc.).
pub async fn log_requests(request: Request<Body>, next: Next) -> Response {
	let method = request.method().clone();
	let path = request.uri().path().to_owned();

	// Check if the method is one that typically mutates state.
	if method == Method::POST
		|| method == Method::DELETE
		|| method == Method::PUT
		|| method == Method::PATCH
	{
		// Log mutating requests as INFO.
		log(LogLevel::Info, &format!("➜ {} {}", method, path));
	} else {
		// Log non-mutating (read-only) requests as DEBUG.
		log(LogLevel::Debug, &format!("➜ {} {}", method, path));
	}

	next.run(request).await
}

#[cfg(test)]
mod tests {
	use super::*;
	use axum::{
		Router,
		http::StatusCode,
		middleware,
		routing::{get, post},
	};
	use tower::util::ServiceExt;

	/// A simple handler to be used as the final destination in the middleware chain.
	async fn handler() -> (StatusCode, &'static str) {
		(StatusCode::OK, "Success")
	}

	/// Tests that a non-mutating method (GET) is processed correctly.
	/// This implicitly verifies that the DEBUG log path is taken without panicking.
	#[tokio::test]
	async fn test_logs_non_mutating_request() {
		let app = Router::new()
			.route("/", get(handler))
			.layer(middleware::from_fn(log_requests));

		let request = Request::builder()
			.method(Method::GET)
			.uri("/")
			.body(Body::empty())
			.unwrap();

		let response = app.oneshot(request).await.unwrap();

		assert_eq!(response.status(), StatusCode::OK);
	}

	/// Tests that a mutating method (POST) is processed correctly.
	/// This implicitly verifies that the INFO log path is taken without panicking.
	#[tokio::test]
	async fn test_logs_mutating_request() {
		let app = Router::new()
			.route("/", post(handler))
			.layer(middleware::from_fn(log_requests));

		let request = Request::builder()
			.method(Method::POST)
			.uri("/")
			.body(Body::empty())
			.unwrap();

		let response = app.oneshot(request).await.unwrap();

		assert_eq!(response.status(), StatusCode::OK);
	}
}
