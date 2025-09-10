/* src/error.rs */

use crate::config;
use axum::{
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use axum_extra::typed_header::TypedHeaderRejection;
use fancy_log::{LogLevel, log};
use std::fs;

pub enum VaneError {
    HostNotFound,
    NoRouteFound,
    BadGateway(anyhow::Error),
    AmbiguousRoute, // New error for ambiguous routing configurations.
}

/// A helper function to read and serve a status page.
pub fn serve_status_page(status: StatusCode, default_message: &str) -> Response {
    if let Ok((_, config_dir)) = config::get_config_paths() {
        let file_path = config_dir
            .join("status")
            .join(format!("{}.html", status.as_u16()));

        if let Ok(body) = fs::read_to_string(&file_path) {
            return (
                status,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                body,
            )
                .into_response();
        } else {
            log(
                LogLevel::Warn,
                &format!(
                    "Could not read status page at {:?}. Serving plain text fallback.",
                    file_path
                ),
            );
        }
    }
    (status, default_message.to_string()).into_response()
}

impl From<TypedHeaderRejection> for VaneError {
    fn from(rejection: TypedHeaderRejection) -> Self {
        log(
            LogLevel::Warn,
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
                log(LogLevel::Error, &format!("Upstream error: {}", e));
                serve_status_page(StatusCode::BAD_GATEWAY, "Upstream server error")
            }
            // Handle the new error by serving a 500 page.
            VaneError::AmbiguousRoute => {
                log(
                    LogLevel::Error,
                    "Request matched multiple routes with same priority. Check configuration.",
                );
                serve_status_page(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Ambiguous route configuration",
                )
            }
        }
    }
}
