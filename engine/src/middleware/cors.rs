/* engine/src/middleware/cors.rs */

use axum::http::HeaderValue;
use std::env;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};

pub fn create_cors_layer() -> CorsLayer {
	let mut allowed_origins = vec![
		"https://dash.vaneproxy.com".to_string(),
		"http://dash.vaneproxy.com".to_string(),
		"http://localhost".to_string(),
	];

	if let Ok(extra_origin) = env::var("CORS") {
		allowed_origins.push(extra_origin); // Own string
	}

	// AllowOrigin::list
	let origin_list = allowed_origins
		.into_iter()
		.map(|s| s.parse::<HeaderValue>().unwrap())
		.collect::<Vec<_>>();

	CorsLayer::new()
		.allow_origin(AllowOrigin::list(origin_list))
		.allow_methods(Any)
		.allow_headers(Any)
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::env;

	#[test]
	fn test_create_cors_layer_default() {
		unsafe { env::remove_var("CORS") };
		let cors = create_cors_layer();
		let _ = cors;
	}

	#[test]
	fn test_create_cors_layer_with_custom_origin() {
		unsafe { env::set_var("CORS", "http://custom-origin.com") };
		let cors = create_cors_layer();
		let _ = cors;

		unsafe { env::remove_var("CORS") };
	}
}
