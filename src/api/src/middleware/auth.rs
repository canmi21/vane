/* src/api/middleware/auth.rs */

use axum::{extract::Request, http::StatusCode, middleware::Next, response::Response};
use fancy_log::{LogLevel, log};

/// Middleware to enforce ACCESS_TOKEN authentication for all management API endpoints.
///
/// This middleware requires the `Authorization: Bearer <token>` header to be present
/// and match the configured ACCESS_TOKEN environment variable.
///
/// Note: This middleware should only be called when ACCESS_TOKEN is configured.
/// If ACCESS_TOKEN is not set, the management console should not be started at all.
pub async fn require_access_token(req: Request, next: Next) -> Result<Response, StatusCode> {
	let expected_token = envflag::get_string("ACCESS_TOKEN", "");

	// Defensive check: This should never happen if bootstrap logic is correct
	if expected_token.is_empty() {
		log(LogLevel::Error, "✗ BUG: Auth middleware called but ACCESS_TOKEN not set");
		return Err(StatusCode::INTERNAL_SERVER_ERROR);
	}

	// Extract Authorization header
	let auth_header = req.headers().get("Authorization").and_then(|v| v.to_str().ok());

	match auth_header {
		Some(token) if token == format!("Bearer {expected_token}") => {
			// Token matches - allow request
			Ok(next.run(req).await)
		}
		Some(_) => {
			// Token present but invalid
			log(LogLevel::Warn, "⚠ Unauthorized API access attempt (invalid token)");
			Err(StatusCode::UNAUTHORIZED)
		}
		None => {
			// No Authorization header
			log(LogLevel::Warn, "⚠ Unauthorized API access attempt (missing Authorization header)");
			Err(StatusCode::UNAUTHORIZED)
		}
	}
}

/// Validates ACCESS_TOKEN configuration.
///
/// Returns Ok(Some(token)) if token is valid (16-128 chars)
/// Returns Ok(None) if token is not set (empty)
/// Returns Err(message) if token is set but invalid (wrong length)
pub fn validate_access_token() -> Result<Option<String>, String> {
	let token = envflag::get_string("ACCESS_TOKEN", "");

	if token.is_empty() {
		return Ok(None);
	}

	let len = token.len();

	if len < 16 {
		return Err(format!("ACCESS_TOKEN too short ({len} chars, requires 16-128 chars)"));
	}

	if len > 128 {
		return Err(format!("ACCESS_TOKEN too long ({len} chars, requires 16-128 chars)"));
	}

	Ok(Some(token))
}
