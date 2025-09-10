/* src/error.rs */

use crate::config;
use axum::{
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use axum_extra::typed_header::TypedHeaderRejection;
use std::fs;

pub enum VaneError {
    HostNotFound,
    NoRouteFound,
    BadGateway(anyhow::Error),
}

/// A helper function to read and serve a status page.
///
/// It attempts to read the corresponding `{code}.html` file from the user's
/// config directory. If the file is not found or cannot be read, it falls back
/// to a plain-text default response.
pub fn serve_status_page(status: StatusCode, default_message: &str) -> Response {
    // Attempt to get the configuration directory path.
    if let Ok((_, config_dir)) = config::get_config_paths() {
        let file_path = config_dir
            .join("status")
            .join(format!("{}.html", status.as_u16()));

        // Try to read the HTML file.
        if let Ok(body) = fs::read_to_string(&file_path) {
            // If successful, return the HTML content with the correct content type.
            return (
                status,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                body,
            )
                .into_response();
        } else {
            // Log a warning if the custom page couldn't be read.
            fancy_log::log(
                fancy_log::LogLevel::Warn,
                &format!(
                    "Could not read status page at {:?}. Serving plain text fallback.",
                    file_path
                ),
            );
        }
    }

    // Fallback response if the config path or file is unavailable.
    (status, default_message.to_string()).into_response()
}

impl From<TypedHeaderRejection> for VaneError {
    fn from(rejection: TypedHeaderRejection) -> Self {
        fancy_log::log(
            fancy_log::LogLevel::Warn,
            &format!("Invalid or missing Host header: {}", rejection),
        );
        VaneError::HostNotFound
    }
}

impl IntoResponse for VaneError {
    fn into_response(self) -> Response {
        match self {
            VaneError::HostNotFound => serve_status_page(
                StatusCode::BAD_REQUEST,
                "Host not configured or header missing",
            ),
            VaneError::NoRouteFound => {
                serve_status_page(StatusCode::NOT_FOUND, "No route found for this path")
            }
            VaneError::BadGateway(e) => {
                fancy_log::log(
                    fancy_log::LogLevel::Error,
                    &format!("Upstream error: {}", e),
                );
                serve_status_page(StatusCode::BAD_GATEWAY, "Upstream server error")
            }
        }
    }
}
