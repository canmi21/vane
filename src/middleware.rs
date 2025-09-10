/* src/middleware.rs */

use crate::{error, models::HttpOptions, state::AppState};
use anyhow::Result;
use axum::http::header::HOST;
use axum::{
    body::Body,
    extract::{ConnectInfo, State},
    http::{Request, Response, StatusCode},
    middleware::Next,
};
use axum_extra::extract::Host;
use fancy_log::{LogLevel, log};
use lazy_limit::limit;
use std::net::SocketAddr;
use std::sync::Arc;

/// Two-layer rate limiting middleware with correct stateful logic.
pub async fn rate_limit_handler(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<Arc<AppState>>,
    Host(host): Host,
    mut req: Request<Body>,
    next: Next,
) -> Result<Response<Body>, StatusCode> {
    let ip = addr.ip();
    let ip_str = ip.to_string();
    let path = req.uri().path().to_owned();

    log(
        LogLevel::Debug,
        &format!("Start check for IP: {}, Path: {}", ip_str, path),
    );

    // --- Layer 1: Mandatory Shield (lazy-limit) ---
    if !limit!(&ip_str, "/shield").await {
        log(
            LogLevel::Debug,
            &format!("FAILED Shield check for IP: {}", ip_str),
        );
        return Ok(error::serve_status_page(
            StatusCode::TOO_MANY_REQUESTS,
            "Too Many Requests",
        ));
    }
    log(
        LogLevel::Debug,
        &format!("PASSED Shield check for IP: {}", ip_str),
    );

    // --- Layer 2: User-Configurable Limits (Governor) ---
    req.extensions_mut().insert(addr);

    if let Some(_domain_config) = state.config.domains.get(&host) {
        // The key to look up is the hostname followed by the request path.
        let full_route_key = format!("{}{}", host, &path);

        // --- Stage 1: Check for an override rule ---
        // Find the longest matching prefix key in the override limiters map.
        let override_key = state
            .override_limiters
            .keys()
            .filter(|k| full_route_key.starts_with(*k))
            .max_by_key(|k| k.len());

        if let Some(key) = override_key {
            // A matching override rule was found in the config.
            if let Some(limiter) = state.override_limiters.get(key) {
                log(LogLevel::Debug, &format!("Matched override rule '{}'", key));
                if limiter.check_key(&ip).is_err() {
                    log(
                        LogLevel::Debug,
                        &format!("FAILED Override rule for IP: {}", ip_str),
                    );
                    return Ok(error::serve_status_page(
                        StatusCode::TOO_MANY_REQUESTS,
                        "Too Many Requests",
                    ));
                }
                log(
                    LogLevel::Debug,
                    &format!(
                        "PASSED Override rule for IP: {}. Request authorized.",
                        ip_str
                    ),
                );
                return Ok(next.run(req).await);
            }
        }

        // --- Stage 2: Check route-specific rules ---
        let route_key = state
            .route_limiters
            .keys()
            .filter(|k| full_route_key.starts_with(*k))
            .max_by_key(|k| k.len());

        if let Some(key) = route_key {
            // A matching route rule was found.
            if let Some(limiter) = state.route_limiters.get(key) {
                log(LogLevel::Debug, &format!("Matched route rule '{}'", key));
                if limiter.check_key(&ip).is_err() {
                    log(
                        LogLevel::Debug,
                        &format!("FAILED Route rule for IP: {}", ip_str),
                    );
                    return Ok(error::serve_status_page(
                        StatusCode::TOO_MANY_REQUESTS,
                        "Too Many Requests",
                    ));
                }
                log(
                    LogLevel::Debug,
                    &format!(
                        "PASSED Route rule for IP: {}. Continuing to default check.",
                        ip_str
                    ),
                );
            }
        }

        // --- Stage 3: Check the default global rule ---
        // This runs for all requests not handled by an override.
        log(
            LogLevel::Debug,
            &format!("Checking default rule for IP: {}", ip_str),
        );
        if state.configurable_limiter.check_key(&ip).is_err() {
            log(
                LogLevel::Debug,
                &format!("FAILED Default rule for IP: {}", ip_str),
            );
            return Ok(error::serve_status_page(
                StatusCode::TOO_MANY_REQUESTS,
                "Too Many Requests",
            ));
        }
        log(
            LogLevel::Debug,
            &format!("PASSED Default rule for IP: {}", ip_str),
        );
    } else {
        log(
            LogLevel::Debug,
            &format!(
                "No domain config found for host '{}'. Skipping configurable limits.",
                host
            ),
        );
    }

    log(
        LogLevel::Debug,
        &format!(
            "All checks passed for IP: {}. Proceeding with request.",
            ip_str
        ),
    );
    Ok(next.run(req).await)
}

// --- Other middleware functions (unchanged) ---

pub async fn alt_svc_handler(
    State(state): State<Arc<AppState>>,
    Host(host): Host,
    req: Request<Body>,
    next: Next,
) -> Response<Body> {
    let mut res = next.run(req).await;

    if let Some(domain_config) = state.config.domains.get(&host) {
        if domain_config.https && domain_config.http3 {
            let port = state.config.https_port;
            let alt_svc_header = format!(r#"h3=":{port}"; ma=86400"#);
            res.headers_mut()
                .insert("Alt-Svc", alt_svc_header.parse().unwrap());
        }
    }
    res
}

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
        HttpOptions::Reject => error::serve_status_page(
            StatusCode::UPGRADE_REQUIRED,
            "HTTP is not supported for this domain. Please use HTTPS.",
        ),
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

pub async fn inject_host_header(mut req: Request<Body>, next: Next) -> Response<Body> {
    if req.headers().get(HOST).is_none() {
        let authority = req.uri().authority().cloned();
        if let Some(authority) = authority {
            req.headers_mut()
                .insert(HOST, authority.as_str().parse().unwrap());
        }
    }
    next.run(req).await
}
