/* src/proxy.rs */

use crate::{error::VaneError, routing, state::AppState};
use axum::{
    body::{Body, to_bytes},
    extract::State,
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
    let path = req.uri().path().to_owned();

    // Find the ordered list of target URLs for the matched route.
    let target_urls =
        routing::find_target_urls(host_str, &path, &state)?.ok_or(VaneError::NoRouteFound)?;

    let client_ip = req
        .extensions()
        .get::<SocketAddr>()
        .map(|addr| addr.ip().to_string());

    // --- START OF CORRECTION ---
    //
    // 1. Preserve the path and query from the original request by copying it into an owned `String`.
    //    This avoids borrowing `req`, allowing us to consume it later with `req.into_parts()`.
    //    - `.map(|pq| pq.as_str())` converts `Option<&PathAndQuery>` to `Option<&str>`.
    //    - `.unwrap_or("/")` provides a default `&str` if there's no path and query.
    //    - `.to_owned()` creates a new `String` from the `&str`, releasing the borrow on `req`.
    let original_path_and_query = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/")
        .to_owned();
    //
    // --- END OF CORRECTION ---

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
        // Clone the request parts and body for each attempt.
        let mut attempt_parts = parts.clone();
        let attempt_body = Body::from(body_bytes.clone());

        // 2. Construct the full target URL.
        let full_target_url = format!(
            "{}{}",
            target_url.strip_suffix('/').unwrap_or(&target_url),
            &original_path_and_query // `format!` can borrow the String as `&str`
        );

        log(
            LogLevel::Debug,
            &format!(
                "Attempting to proxy request for {} to target: {}",
                host_str,
                full_target_url // Log using the fully constructed URL
            ),
        );

        // 3. Parse the fully constructed URL.
        let target_uri = match full_target_url.parse() {
            Ok(uri) => uri,
            Err(e) => {
                last_error = Some(anyhow::anyhow!(
                    "Invalid constructed target URL '{}': {}",
                    full_target_url,
                    e
                ));
                continue; // Try the next target
            }
        };

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

        // Use the newly constructed URI which includes the correct path
        attempt_parts.uri = target_uri;
        attempt_parts.version = Version::HTTP_11;

        let attempt_req = Request::from_parts(attempt_parts, attempt_body);

        // Send the request to the current target.
        match state.http_client.request(attempt_req).await {
            Ok(response) => {
                if !response.status().is_server_error() {
                    log(
                        LogLevel::Debug,
                        &format!(
                            "Successfully proxied to {}. Returning response.",
                            full_target_url // Use the full URL in the success log
                        ),
                    );
                    return Ok(response.map(Body::new));
                }

                log(
                    LogLevel::Warn,
                    &format!(
                        "Target {} returned server error: {}. Trying next target.",
                        full_target_url, // Use the full URL in the warning log
                        response.status()
                    ),
                );
                last_error = Some(anyhow::anyhow!(
                    "Target '{}' failed with status {}",
                    full_target_url, // Use the full URL in the error message
                    response.status()
                ));
            }
            Err(e) => {
                log(
                    LogLevel::Warn,
                    &format!(
                        "Connection to target {} failed: {}. Trying next target.",
                        full_target_url,
                        e // Use the full URL in the connection error log
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
