/* src/proxy.rs */

use crate::{error::VaneError, routing, state::AppState};
use axum::{
    body::Body,
    extract::State,
    http::{Request, Version},
    response::Response,
};
use axum_extra::typed_header::TypedHeader;
use headers::Host;
use std::sync::Arc;

#[axum::debug_handler]
pub async fn proxy_handler(
    State(state): State<Arc<AppState>>,
    TypedHeader(host): TypedHeader<Host>,
    req: Request<Body>,
) -> Result<Response, VaneError> {
    let host_str = host.hostname();
    let path = req.uri().path();

    // 1. Find the target backend URL
    let target_url_str =
        routing::find_target_url(host_str, path, &state).ok_or_else(|| VaneError::NoRouteFound)?;

    // Create a new request but preserve the original body
    let (mut parts, body) = req.into_parts();

    // 2. Modify the request URI to point to the target
    let target_uri = target_url_str.parse().map_err(|e| {
        VaneError::BadGateway(anyhow::anyhow!(
            "Invalid target URL '{}': {}",
            target_url_str,
            e
        ))
    })?;

    parts.uri = target_uri;

    // ----- THE FIX IS HERE -----
    // Corrected: Force the outgoing request to be HTTP/1.1.
    // This decouples the client-facing protocol (H2/H3) from the
    // backend-facing protocol, which is what a reverse proxy should do.
    parts.version = Version::HTTP_11;
    // ---------------------------

    let req = Request::from_parts(parts, body);

    // 3. Send the request to the target backend
    fancy_log::log(
        fancy_log::LogLevel::Debug,
        &format!("Proxying request for {} to {}", host_str, target_url_str),
    );

    let response = state
        .http_client
        .request(req)
        .await
        .map_err(|e| VaneError::BadGateway(e.into()))?;

    // 4. Return the response from the backend to the original client
    Ok(response.map(Body::new))
}
