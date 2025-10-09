/* src/proxy.rs */

use crate::{error::VaneError, routing, state::AppState};
use axum::{
    body::{Body, to_bytes},
    extract::State,
    http::{Request, StatusCode, Version, header},
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

const IP_HEADERS_TO_CLEAN: &[&str] = &[
    "x-real-ip",
    "x-forwarded-for",
    "x-forwarded",
    "forwarded-for",
    "forwarded",
];

/// Checks if a request has the required headers for a WebSocket upgrade.
fn is_websocket_upgrade(req: &Request<Body>) -> bool {
    req.headers()
        .get(header::CONNECTION)
        .and_then(|v| v.to_str().ok())
        .map(|s| {
            s.split(',')
                .any(|part| part.trim().eq_ignore_ascii_case("upgrade"))
        })
        .unwrap_or(false)
        && req
            .headers()
            .get(header::UPGRADE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.eq_ignore_ascii_case("websocket"))
            .unwrap_or(false)
}

/// Spawns a background task to proxy a bidirectional TCP stream.
fn spawn_websocket_proxy(client_upgraded: OnUpgrade, backend_upgraded: OnUpgrade) {
    tokio::spawn(async move {
        match tokio::try_join!(client_upgraded, backend_upgraded) {
            Ok((client_upgraded, backend_upgraded)) => {
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

#[axum::debug_handler]
pub async fn proxy_handler(
    State(state): State<Arc<AppState>>,
    TypedHeader(host): TypedHeader<Host>,
    mut req: Request<Body>,
) -> Result<Response, VaneError> {
    let host_str = host.hostname();
    let path = req.uri().path().to_owned();

    let matched_route =
        routing::find_matched_route(host_str, &path, &state)?.ok_or(VaneError::NoRouteFound)?;

    // --- BRANCH 1: WEBSOCKET PROXY LOGIC ---
    if matched_route.websocket && is_websocket_upgrade(&req) {
        log(
            LogLevel::Debug,
            &format!("Attempting WebSocket upgrade for {} {}", host_str, path),
        );

        let target_url = matched_route
            .targets
            .first()
            .ok_or(VaneError::NoRouteFound)?;

        // FIX: `hyper::upgrade::on` returns `OnUpgrade` directly.
        // It does not return a `Result` and cannot be used with `?`.
        // Error handling happens when we `.await` this future.
        let on_upgrade = hyper::upgrade::on(&mut req);

        // FIX: Modify the existing request instead of creating a new one.
        // This preserves all necessary headers and extensions.
        let original_uri = req.uri().clone();
        let (parts, query) = (original_uri.parts(), original_uri.query());

        // FIX: Explicitly parse into `axum::http::Uri` to fix type inference error.
        let mut uri_builder = target_url
            .parse::<axum::http::Uri>()
            .map_err(|e| VaneError::BadGateway(anyhow::anyhow!(e)))?
            .into_parts();

        // Preserve the original path and query.
        uri_builder.path_and_query = parts.path_and_query.clone();
        if query.is_some() {
            let new_path_and_query = format!(
                "{}?{}",
                uri_builder.path_and_query.unwrap().as_str(),
                query.unwrap()
            );
            uri_builder.path_and_query = Some(new_path_and_query.parse().unwrap());
        }

        let new_uri = axum::http::Uri::from_parts(uri_builder)
            .map_err(|e| VaneError::BadGateway(anyhow::anyhow!(e)))?;

        // Update the URI of the original request.
        *req.uri_mut() = new_uri;

        let backend_response = state.http_client.request(req).await;

        match backend_response {
            Ok(mut res) if res.status() == StatusCode::SWITCHING_PROTOCOLS => {
                // FIX: Same as above, `hyper::upgrade::on` returns the future directly.
                let backend_on_upgrade = hyper::upgrade::on(&mut res);
                spawn_websocket_proxy(on_upgrade, backend_on_upgrade);
                Ok(res.into_response())
            }
            Ok(res) => {
                log(
                    LogLevel::Warn,
                    &format!(
                        "WebSocket upgrade failed. Backend for {} returned status: {}",
                        host_str,
                        res.status()
                    ),
                );
                Ok(res.into_response())
            }
            Err(e) => Err(VaneError::BadGateway(e.into())),
        }
    }
    // --- BRANCH 2: STANDARD HTTP PROXY LOGIC (FAILOVER SUPPORT) ---
    else {
        let client_ip = req
            .extensions()
            .get::<SocketAddr>()
            .map(|addr| addr.ip().to_string());

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

            log(
                LogLevel::Debug,
                &format!(
                    "Attempting to proxy request for {} to target: {}",
                    host_str, full_target_url
                ),
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

            for header in IP_HEADERS_TO_CLEAN {
                attempt_parts.headers.remove(*header);
            }
            if let Some(ip) = &client_ip {
                attempt_parts
                    .headers
                    .insert("X-Forwarded-For", ip.parse().unwrap());
            }

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
}
