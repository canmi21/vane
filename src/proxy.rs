/* src/proxy.rs */

use crate::{error::VaneError, routing, state::AppState};
use axum::{
    body::{Body, to_bytes},
    extract::State,
    http::{HeaderMap, Request, StatusCode, Version, header},
    response::{IntoResponse, Response},
};
use axum_extra::typed_header::TypedHeader;
use fancy_log::{LogLevel, log};
use headers::Host;
use hyper::upgrade::OnUpgrade;
use hyper_util::rt::tokio::TokioIo;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::copy_bidirectional;

// FIX: This constant will now be used by a helper function.
const IP_HEADERS_TO_CLEAN: &[&str] = &[
    "x-real-ip",
    "x-forwarded-for",
    "x-forwarded",
    "forwarded-for",
    "forwarded",
];

/// Checks if a request has the required headers for a WebSocket upgrade.
fn is_websocket_upgrade(req: &Request<Body>) -> bool {
    let is_upgrade = req
        .headers()
        .get(header::CONNECTION)
        .and_then(|v| v.to_str().ok())
        .map(|s| {
            s.split(',')
                .any(|part| part.trim().eq_ignore_ascii_case("upgrade"))
        })
        .unwrap_or(false);

    let is_websocket = req
        .headers()
        .get(header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false);

    is_upgrade && is_websocket
}

/// Spawns a background task to proxy a bidirectional TCP stream.
fn spawn_websocket_proxy(client_upgraded: OnUpgrade, backend_upgraded: OnUpgrade) {
    tokio::spawn(async move {
        match tokio::try_join!(client_upgraded, backend_upgraded) {
            Ok((client_upgraded, backend_upgraded)) => {
                log(
                    LogLevel::Debug,
                    "Successfully upgraded both client and backend connections.",
                );
                let mut client_socket = TokioIo::new(client_upgraded);
                let mut backend_socket = TokioIo::new(backend_upgraded);

                if let Err(e) = copy_bidirectional(&mut client_socket, &mut backend_socket).await {
                    log(
                        LogLevel::Debug,
                        &format!("WebSocket proxy stream ended: {}", e),
                    );
                }
            }
            Err(e) => {
                log(
                    LogLevel::Warn,
                    &format!("WebSocket connection upgrade failed: {}", e),
                );
            }
        }
    });
}

// NEW: Helper function to sanitize IP headers and set the correct forwarded IP.
fn sanitize_headers_and_set_forwarded_ip(headers: &mut HeaderMap, client_ip: Option<&String>) {
    // Remove any IP-related headers from the incoming request to prevent spoofing.
    for header_name in IP_HEADERS_TO_CLEAN {
        headers.remove(*header_name);
    }

    // Add the real client IP as the X-Forwarded-For header.
    if let Some(ip) = client_ip {
        if let Ok(ip_header) = ip.parse() {
            headers.insert("X-Forwarded-For", ip_header);
        }
    }
}

#[axum::debug_handler]
pub async fn proxy_handler(
    State(state): State<Arc<AppState>>,
    TypedHeader(host): TypedHeader<Host>,
    mut req: Request<Body>,
) -> Result<Response, VaneError> {
    let host_str = host.hostname();
    let path = req.uri().path().to_owned();
    log(
        LogLevel::Debug,
        &format!(
            "Proxy handler received request for host '{}', path '{}'",
            host_str, path
        ),
    );

    let client_ip = req
        .extensions()
        .get::<SocketAddr>()
        .map(|addr| addr.ip().to_string());

    let matched_route =
        routing::find_matched_route(host_str, &path, &state)?.ok_or(VaneError::NoRouteFound)?;
    log(
        LogLevel::Debug,
        &format!(
            "Matched route: path='{}', websocket={}",
            matched_route.path, matched_route.websocket
        ),
    );

    // --- BRANCH 1: WEBSOCKET PROXY LOGIC ---
    if matched_route.websocket && is_websocket_upgrade(&req) {
        log(LogLevel::Debug, "Entering WebSocket proxy branch.");

        let target_url = matched_route
            .targets
            .first()
            .ok_or(VaneError::NoRouteFound)?;
        log(
            LogLevel::Debug,
            &format!("Selected WebSocket target: {}", target_url),
        );

        let on_upgrade = hyper::upgrade::on(&mut req);

        let mut target_parts = target_url
            .parse::<axum::http::Uri>()
            .map_err(|e| VaneError::BadGateway(anyhow::anyhow!("Invalid target URL: {}", e)))?
            .into_parts();

        let original_parts = req.uri().clone().into_parts();
        target_parts.path_and_query = original_parts.path_and_query;

        let new_uri = axum::http::Uri::from_parts(target_parts).map_err(|e| {
            VaneError::BadGateway(anyhow::anyhow!("Failed to build proxy URI: {}", e))
        })?;
        log(
            LogLevel::Debug,
            &format!("Forwarding WebSocket request to URI: {}", new_uri),
        );

        *req.uri_mut() = new_uri;

        // FIX: Sanitize headers before forwarding the WebSocket upgrade request.
        sanitize_headers_and_set_forwarded_ip(req.headers_mut(), client_ip.as_ref());

        let backend_response = state.http_client.request(req).await;

        match backend_response {
            Ok(mut res) => {
                log(
                    LogLevel::Debug,
                    &format!("Backend responded with status: {}", res.status()),
                );
                if res.status() == StatusCode::SWITCHING_PROTOCOLS {
                    let backend_on_upgrade = hyper::upgrade::on(&mut res);
                    spawn_websocket_proxy(on_upgrade, backend_on_upgrade);
                }
                Ok(res.into_response())
            }
            Err(e) => {
                log(
                    LogLevel::Error,
                    &format!("Error connecting to WebSocket backend: {}", e),
                );
                Err(VaneError::BadGateway(e.into()))
            }
        }
    }
    // --- BRANCH 2: STANDARD HTTP PROXY LOGIC (FAILOVER SUPPORT) ---
    else {
        log(LogLevel::Debug, "Entering standard HTTP proxy branch.");

        let original_path_and_query = req
            .uri()
            .path_and_query()
            .map(|pq| pq.as_str())
            .unwrap_or("/")
            .to_owned();
        let (parts, body) = req.into_parts();
        let body_bytes = match to_bytes(body, usize::MAX).await {
            Ok(bytes) => bytes,
            Err(e) => return Err(VaneError::BadGateway(e.into())),
        };

        let mut last_error: Option<anyhow::Error> = None;
        for target_url in &matched_route.targets {
            let mut attempt_parts = parts.clone();
            let attempt_body = Body::from(body_bytes.clone());

            let full_target_url = format!(
                "{}{}",
                target_url.strip_suffix('/').unwrap_or(target_url),
                &original_path_and_query
            );
            let target_uri = match full_target_url.parse() {
                Ok(uri) => uri,
                Err(e) => {
                    last_error = Some(anyhow::anyhow!(
                        "Invalid constructed target URL '{}': {}",
                        full_target_url,
                        e
                    ));
                    continue;
                }
            };

            // FIX: Use the helper function to properly sanitize headers for the HTTP request.
            sanitize_headers_and_set_forwarded_ip(&mut attempt_parts.headers, client_ip.as_ref());

            attempt_parts.uri = target_uri;
            attempt_parts.version = Version::HTTP_11;

            let attempt_req = Request::from_parts(attempt_parts, attempt_body);

            match state.http_client.request(attempt_req).await {
                Ok(response) => {
                    if !response.status().is_server_error() {
                        return Ok(response.map(Body::new));
                    }
                    last_error = Some(anyhow::anyhow!(
                        "Target '{}' failed with status {}",
                        full_target_url,
                        response.status()
                    ));
                }
                Err(e) => {
                    last_error = Some(e.into());
                }
            }
        }

        Err(VaneError::BadGateway(last_error.unwrap_or_else(|| {
            anyhow::anyhow!("No available backend targets could handle the request.")
        })))
    }
}
