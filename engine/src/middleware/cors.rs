/* engine/src/middleware/cors.rs */

use axum::http;
use std::env;
use tower_http::cors::{Any, CorsLayer};

/// Creates a CORS middleware layer with a configurable origin.
pub fn create_cors_layer() -> CorsLayer {
	let allowed_origin = env::var("CORS").unwrap_or_else(|_| "https://canmi.net".to_string());

	CorsLayer::new()
		// Allow requests from the configured origin.
		.allow_origin(allowed_origin.parse::<http::HeaderValue>().unwrap())
		// Allow all common HTTP methods.
		.allow_methods(Any)
		// Allow all headers.
		.allow_headers(Any)
}
