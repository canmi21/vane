/* src/middleware.rs */

use crate::{models::HttpOptions, state::AppState};
use axum::{
    body::Body,
    extract::State,
    http::{Request, Response, StatusCode},
    middleware::Next,
    response::IntoResponse,
};
use axum_extra::extract::Host;
use std::sync::Arc;

/// Middleware for the HTTP server to handle domain-specific options.
pub async fn http_request_handler(
    State(state): State<Arc<AppState>>,
    Host(host): Host,
    req: Request<Body>,
    next: Next,
) -> Response<Body> {
    // Corrected: Specify the body type Response<Body>
    let domain_config = match state.config.domains.get(&host) {
        Some(config) => config,
        None => {
            // If the host is not configured, let it fall through to the proxy_handler,
            // which will then return a proper VaneError::HostNotFound.
            return next.run(req).await;
        }
    };

    match domain_config.http_options {
        // For "allow", just proceed to the proxy handler.
        HttpOptions::Allow => next.run(req).await,

        // For "reject", return a 426 Upgrade Required response.
        HttpOptions::Reject => (
            StatusCode::UPGRADE_REQUIRED,
            "HTTP is not supported for this domain. Please use HTTPS.",
        )
            .into_response(),

        // For "upgrade", build a redirection response to the HTTPS version of the URL.
        HttpOptions::Upgrade => {
            let uri = format!(
                "https://{}{}",
                host,
                req.uri()
                    .path_and_query()
                    .map(|pq| pq.as_str())
                    .unwrap_or("/")
            );
            Response::builder()
                .status(StatusCode::MOVED_PERMANENTLY)
                .header("Location", uri)
                .body(Body::empty())
                .unwrap() // This unwrap is safe as we are building a valid response.
        }
    }
}

/// Middleware for the HTTPS server to add the HSTS header if configured.
pub async fn hsts_handler(
    State(state): State<Arc<AppState>>,
    Host(host): Host,
    req: Request<Body>,
    next: Next,
) -> Response<Body> {
    // Corrected: Specify the body type Response<Body>
    // Run the proxy handler first to get the response.
    let mut res = next.run(req).await;

    // Check if HSTS is enabled for this host.
    if let Some(domain_config) = state.config.domains.get(&host) {
        if domain_config.https && domain_config.hsts {
            // Add the HSTS header. A common value is one year.
            // This unwrap is safe as the header value is valid.
            res.headers_mut().insert(
                "Strict-Transport-Security",
                "max-age=31536000; includeSubDomains".parse().unwrap(),
            );
        }
    }

    res
}
