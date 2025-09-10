/* src/middleware.rs */

use crate::{error, models::HttpOptions, ratelimit, state::AppState};
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
use tower::{ServiceBuilder, ServiceExt};
use tower_http::cors::{Any, CorsLayer};

/// Dynamic CORS middleware that applies the correct policy based on the request's host.
pub async fn cors_handler(
    State(state): State<Arc<AppState>>,
    Host(host): Host,
    req: Request<Body>,
    next: Next,
) -> Response<Body> {
    if let Some(domain_config) = state.config.domains.get(&host) {
        // Check if a CORS configuration exists for this domain.
        if let Some(cors_config) = &domain_config.cors {
            log(
                LogLevel::Debug,
                &format!("Applying CORS policy for host '{}'", host),
            );

            let mut cors_layer = CorsLayer::new()
                .allow_methods(Any) // Allow all common methods.
                .allow_headers(Any); // Allow all common headers.

            // Configure allowed origins based on the config file.
            if cors_config.allowed_origins.contains(&"*".to_string()) {
                cors_layer = cors_layer.allow_origin(Any);
            } else {
                // Parse the origins from the config.
                let origins: Vec<_> = cors_config
                    .allowed_origins
                    .iter()
                    .filter_map(|s| s.parse().ok())
                    .collect();
                cors_layer = cors_layer.allow_origin(origins);
            }

            // Dynamically create a Tower service with the configured CorsLayer
            // and call it with the request and the `next` service using .oneshot().
            let result = ServiceBuilder::new()
                .layer(cors_layer)
                .service(next)
                .oneshot(req) // .oneshot() is now available from the ServiceExt trait.
                .await;

            // The result from .oneshot() is a Result; we handle the error case
            // instead of calling .unwrap() to avoid a potential panic.
            return match result {
                Ok(response) => response,
                Err(_) => {
                    // This case is unlikely with axum's 'Next' service, but it's robust to handle it.
                    error::serve_status_page(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Internal server error in CORS middleware",
                    )
                }
            };
        }
    }

    // If no CORS config is found for this host, just pass the request through.
    log(
        LogLevel::Debug,
        &format!("No CORS policy for host '{}'. Passing through.", host),
    );
    next.run(req).await
}

/// Two-layer rate limiting middleware using the refactored ratelimit module.
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

    req.extensions_mut().insert(addr);

    if state.config.domains.get(&host).is_some() {
        let full_path_key = format!("{}{}", host, &path);

        if let Some(found_match) =
            ratelimit::find_best_match(&state.override_limiters, &full_path_key)
        {
            log(
                LogLevel::Debug,
                &format!(
                    "Applying best override match from pattern '{}'",
                    found_match.pattern
                ),
            );
            if found_match.limiter.check_key(&ip).is_err() {
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

        if let Some(found_match) = ratelimit::find_best_match(&state.route_limiters, &full_path_key)
        {
            log(
                LogLevel::Debug,
                &format!(
                    "Applying best route match from pattern '{}'",
                    found_match.pattern
                ),
            );
            if found_match.limiter.check_key(&ip).is_err() {
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
                "No domain config for host '{}'. Skipping configurable limits.",
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

/// Middleware for the HTTPS (TCP) server to add the Alt-Svc header for HTTP/3 discovery.
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

/// Middleware for the HTTP server to handle domain-specific options.
pub async fn http_request_handler(
    State(state): State<Arc<AppState>>,
    Host(host): Host,
    req: Request<Body>,
    next: Next,
) -> Response<Body> {
    let domain_config = match state.config.domains.get(&host) {
        Some(config) => config,
        None => return next.run(req).await,
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

/// Injects the `Host` header from the URI's authority if it's missing.
pub async fn inject_host_header(mut req: Request<Body>, next: Next) -> Response<Body> {
    if req.headers().get(HOST).is_none() {
        if let Some(authority) = req.uri().authority().cloned() {
            req.headers_mut()
                .insert(HOST, authority.as_str().parse().unwrap());
        }
    }
    next.run(req).await
}
