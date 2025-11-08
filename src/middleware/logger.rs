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
