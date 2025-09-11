/* src/proxy.rs */

use crate::{error::VaneError, routing, state::AppState};
use axum::{
    body::{Body, to_bytes},
    extract::State,
    // MODIFIED: Removed unused StatusCode import.
    http::{Request, Version},
    response::Response,
};
use axum_extra::typed_header::TypedHeader;
use fancy_log::{LogLevel, log};
use headers::Host;
use std::net::SocketAddr;
use std::sync::Arc;

const IP_HEADERS_TO_CLEAN: &[&str] = &[
    "x-real-ip",
    "x-forwarded-for",
    "x-forwarded",
    "forwarded-for",
    "forwarded",
];

#[axum::debug_handler]
pub async fn proxy_handler(
    State(state): State<Arc<AppState>>,
    TypedHeader(host): TypedHeader<Host>,
    req: Request<Body>,
) -> Result<Response, VaneError> {
    let host_str = host.hostname();
    // MODIFIED: Clone the path to an owned String to avoid borrow checker errors.
    let path = req.uri().path().to_owned();

    // Find the ordered list of target URLs for the matched route.
    let target_urls =
        routing::find_target_urls(host_str, &path, &state)?.ok_or(VaneError::NoRouteFound)?;

    let client_ip = req
        .extensions()
        .get::<SocketAddr>()
        .map(|addr| addr.ip().to_string());

    // MODIFIED: Removed `mut` from `parts` as it is not needed.
    let (parts, body) = req.into_parts();

    // Buffer the body so it can be reused for each failover attempt.
    let body_bytes = match to_bytes(body, usize::MAX).await {
        Ok(bytes) => bytes,
        Err(e) => return Err(VaneError::BadGateway(e.into())),
    };

    // --- FAILOVER LOOP ---
    // Iterate through the configured targets in order.
    let mut last_error: Option<anyhow::Error> = None;
    for target_url in target_urls {
        log(
            LogLevel::Debug,
            &format!(
                "Attempting to proxy request for {} to target: {}",
                host_str, target_url
            ),
        );

        // Clone the request parts and body for each attempt.
        let mut attempt_parts = parts.clone();
        let attempt_body = Body::from(body_bytes.clone());

        // Clean any existing IP-related headers to prevent spoofing.
        for header in IP_HEADERS_TO_CLEAN {
            attempt_parts.headers.remove(*header);
        }
        // Add the X-Forwarded-For header with the real client IP.
        if let Some(ip) = &client_ip {
            attempt_parts
                .headers
                .insert("X-Forwarded-For", ip.parse().unwrap());
        }

        // Parse the target URL into a URI.
        let target_uri = match target_url.parse() {
            Ok(uri) => uri,
            Err(e) => {
                last_error = Some(anyhow::anyhow!(
                    "Invalid target URL '{}': {}",
                    target_url,
                    e
                ));
                continue; // Try the next target
            }
        };

        attempt_parts.uri = target_uri;
        attempt_parts.version = Version::HTTP_11;

        let attempt_req = Request::from_parts(attempt_parts, attempt_body);

        // Send the request to the current target.
        match state.http_client.request(attempt_req).await {
            Ok(response) => {
                // A successful response from the backend (even a 4xx client error) means we stop here.
                // We only failover on 5xx server errors.
                if !response.status().is_server_error() {
                    log(
                        LogLevel::Debug,
                        &format!(
                            "Successfully proxied to {}. Returning response.",
                            target_url
                        ),
                    );
                    return Ok(response.map(Body::new));
                }

                // It was a 5xx error, log it and prepare to try the next target.
                log(
                    LogLevel::Warn,
                    &format!(
                        "Target {} returned server error: {}. Trying next target.",
                        target_url,
                        response.status()
                    ),
                );
                last_error = Some(anyhow::anyhow!(
                    "Target '{}' failed with status {}",
                    target_url,
                    response.status()
                ));
            }
            Err(e) => {
                // A connection-level error occurred.
                log(
                    LogLevel::Warn,
                    &format!(
                        "Connection to target {} failed: {}. Trying next target.",
                        target_url, e
                    ),
                );
                last_error = Some(e.into());
            }
        }
    }

    // If the loop finishes, all targets have failed.
    log(
        LogLevel::Error,
        &format!(
            "All backend targets failed for request to {} {}",
            host_str, path
        ),
    );

    Err(VaneError::BadGateway(last_error.unwrap_or_else(|| {
        anyhow::anyhow!("No available backend targets could handle the request.")
    })))
}
