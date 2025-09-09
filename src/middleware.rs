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

/// Middleware for the HTTPS (TCP) server to add the Alt-Svc header for HTTP/3 discovery.
pub async fn alt_svc_handler(
    State(state): State<Arc<AppState>>,
    Host(host): Host,
    req: Request<Body>,
    next: Next,
) -> Response<Body> {
    let mut res = next.run(req).await;

    if let Some(domain_config) = state.config.domains.get(&host) {
        // If this domain has http3 enabled, advertise it.
        if domain_config.https && domain_config.http3 {
            let port = state.config.https_port;
            // The header tells the client that HTTP/3 is available on the same port,
            // and this information is valid for 24 hours (86400 seconds).
            let alt_svc_header = format!(r#"h3=":{port}"; ma=86400"#);
            res.headers_mut()
                .insert("Alt-Svc", alt_svc_header.parse().unwrap());
        }
    }

    res
}

/// Middleware for the HTTP server to handle domain-specific options.
pub async fn http_request_handler(
    State(state): State<Arc<AppState>>,
    Host(host): Host,
    req: Request<Body>,
    next: Next,
) -> Response<Body> {
    let domain_config = match state.config.domains.get(&host) {
        Some(config) => config,
        None => {
            return next.run(req).await;
        }
    };

    match domain_config.http_options {
        HttpOptions::Allow => next.run(req).await,
        HttpOptions::Reject => (
            StatusCode::UPGRADE_REQUIRED,
            "HTTP is not supported for this domain. Please use HTTPS.",
        )
            .into_response(),
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
                .unwrap()
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
    let mut res = next.run(req).await;

    if let Some(domain_config) = state.config.domains.get(&host) {
        if domain_config.https && domain_config.hsts {
            res.headers_mut().insert(
                "Strict-Transport-Security",
                "max-age=31536000; includeSubDomains".parse().unwrap(),
            );
        }
    }

    res
}
