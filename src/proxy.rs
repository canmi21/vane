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
    let path = req.uri().path();

    // The call to find_target_url now returns a Result, which we propagate with `?`.
    // The inner Option is then handled to produce a NoRouteFound error if empty.
    let target_url_str =
        routing::find_target_url(host_str, path, &state)?.ok_or(VaneError::NoRouteFound)?;

    let client_ip = req
        .extensions()
        .get::<SocketAddr>()
        .map(|addr| addr.ip().to_string());

    let (mut parts, body) = req.into_parts();

    // Clean any existing IP-related headers to prevent spoofing.
    for header in IP_HEADERS_TO_CLEAN {
        parts.headers.remove(*header);
    }
    // Add the X-Forwarded-For header with the real client IP.
    if let Some(ip) = client_ip {
        parts.headers.insert("X-Forwarded-For", ip.parse().unwrap());
    }

    let target_uri = target_url_str.parse().map_err(|e| {
        VaneError::BadGateway(anyhow::anyhow!(
            "Invalid target URL '{}': {}",
            target_url_str,
            e
        ))
    })?;

    parts.uri = target_uri;
    parts.version = Version::HTTP_11;

    let req = Request::from_parts(parts, body);

    fancy_log::log(
        fancy_log::LogLevel::Debug,
        &format!("Proxying request for {} to {}", host_str, target_url_str),
    );

    let response = state
        .http_client
        .request(req)
        .await
        .map_err(|e| VaneError::BadGateway(e.into()))?;

    Ok(response.map(Body::new))
}
