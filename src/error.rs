/* src/error.rs */

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use axum_extra::typed_header::TypedHeaderRejection;

pub enum VaneError {
    HostNotFound,
    NoRouteFound,
    BadGateway(anyhow::Error),
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
        let (status, message) = match self {
            VaneError::HostNotFound => (
                StatusCode::BAD_REQUEST,
                "Host not configured or header missing".to_string(),
            ),
            VaneError::NoRouteFound => (
                StatusCode::NOT_FOUND,
                "No route found for this path".to_string(),
            ),
            VaneError::BadGateway(e) => {
                fancy_log::log(
                    fancy_log::LogLevel::Error,
                    &format!("Upstream error: {}", e),
                );
                (StatusCode::BAD_GATEWAY, "Upstream server error".to_string())
            }
        };
        (status, message).into_response()
    }
}
