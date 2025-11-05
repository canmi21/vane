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
	use axum::body::Body;
	use axum::http::{Method, Request};
	use axum::{Router, routing::get};
	use serial_test::serial; // <-- FIX: Import the serial macro.
	use std::env;
	use tower::ServiceExt; // for `oneshot`

	#[tokio::test]
	#[serial] // <-- FIX: Add attribute to serialize test execution.
	async fn test_create_cors_layer_default() {
		// Setup: Ensure CORS is not set.
		unsafe { env::remove_var("CORS") };

		let cors_layer = create_cors_layer();
		let app = Router::new()
			.route("/", get(|| async { "ok" }))
			.layer(cors_layer);

		let req = Request::builder()
			.method(Method::GET)
			.uri("/")
			.header("Origin", "http://localhost")
			.body(Body::empty())
			.unwrap();

		let resp = app.oneshot(req).await.unwrap();
		let headers = resp.headers();
		assert_eq!(
			headers.get("access-control-allow-origin").unwrap(),
			"http://localhost"
		);
	}

	#[tokio::test]
	#[serial]
	async fn test_create_cors_layer_with_custom_origin() {
		// Setup: Set custom CORS origin.
		unsafe { env::set_var("CORS", "http://custom-origin.com") };

		let cors_layer = create_cors_layer();
		let app = Router::new()
			.route("/", get(|| async { "ok" }))
			.layer(cors_layer);

		let req = Request::builder()
			.method(Method::GET)
			.uri("/")
			.header("Origin", "http://custom-origin.com")
			.body(Body::empty())
			.unwrap();

		let resp = app.oneshot(req).await.unwrap();
		let headers = resp.headers();
		assert_eq!(
			headers.get("access-control-allow-origin").unwrap(),
			"http://custom-origin.com"
		);

		// Teardown: Clean up the environment variable.
		unsafe { env::remove_var("CORS") };
	}

	#[tokio::test]
	#[serial] // <-- FIX: Add attribute to serialize test execution.
	async fn test_create_cors_layer_reject_unlisted_origin() {
		// Setup: Ensure CORS is not set.
		unsafe { env::remove_var("CORS") };

		let cors_layer = create_cors_layer();
		let app = Router::new()
			.route("/", get(|| async { "ok" }))
			.layer(cors_layer);

		let req = Request::builder()
			.method(Method::GET)
			.uri("/")
			.header("Origin", "http://not-listed.com")
			.body(Body::empty())
			.unwrap();

		let resp = app.oneshot(req).await.unwrap();
		let headers = resp.headers();

		assert!(headers.get("access-control-allow-origin").is_none());
	}
}
