/* src/middleware.rs */

use crate::{error, models::HttpOptions, ratelimit, state::AppState};
use axum::http::{HeaderValue, Method, header};
use axum::{
    body::Body,
    extract::{ConnectInfo, State},
    http::{Request, Response, StatusCode},
    middleware::Next,
};
use axum_extra::extract::Host;
use fancy_log::{LogLevel, log};
use lazy_limit::limit;
use std::collections::HashSet;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;

/// NEW: Middleware to inject custom 'Server' and 'Proxy' headers into every response.
/// This should be one of the outermost layers to ensure it runs on all responses.
pub async fn inject_response_headers_handler(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
    next: Next,
) -> Response<Body> {
    // Get the response from the inner services first
    let mut res = next.run(req).await;

    // Inject/override the 'Server' header if it's configured in the environment.
    if let Some(server_name) = &state.config.server_header {
        if let Ok(value) = HeaderValue::from_str(server_name) {
            res.headers_mut().insert(header::SERVER, value);
        }
    }

    // Inject the 'Proxy' header with the crate name and version.
    // The CARGO_PKG_VERSION is embedded at compile-time.
    let proxy_value = format!("vane/{}", env!("CARGO_PKG_VERSION"));
    if let Ok(value) = HeaderValue::from_str(&proxy_value) {
        res.headers_mut().insert("proxy", value);
    }

    res
}

/// NEW: Middleware to filter requests based on the HTTP method.
/// This runs early to reject unauthorized methods before further processing.
pub async fn method_filter_handler(
    State(state): State<Arc<AppState>>,
    Host(host): Host,
    req: Request<Body>,
    next: Next,
) -> Response<Body> {
    if let Some(domain_config) = state.config.domains.get(&host) {
        if let Some(methods_config) = &domain_config.methods {
            let allowed_str = methods_config.allow.trim();

            // If "allow" is not a wildcard, check the method.
            if allowed_str != "*" {
                // Parse the allowed methods into a HashSet for efficient lookup.
                let allowed_methods: HashSet<Method> = allowed_str
                    .split(',')
                    .filter_map(|s| Method::from_str(s.trim().to_uppercase().as_str()).ok())
                    .collect();

                // If the request's method is not in the allowed set, reject it.
                if !allowed_methods.contains(req.method()) {
                    log(
                        LogLevel::Warn,
                        &format!(
                            "Method '{}' not allowed for host '{}' by domain config. Rejecting.",
                            req.method(),
                            host
                        ),
                    );
                    return error::serve_status_page(
                        StatusCode::METHOD_NOT_ALLOWED,
                        "Method Not Allowed",
                    );
                }
            }
        }
    }
    // Method is allowed, or no filter is configured. Continue to the next middleware.
    next.run(req).await
}

/// REWRITTEN: A powerful, manually-implemented CORS middleware for fine-grained control.
pub async fn cors_handler(
    State(state): State<Arc<AppState>>,
    Host(host): Host,
    req: Request<Body>,
    next: Next,
) -> Response<Body> {
    // Extract CORS configuration for the current domain.
    let cors_config = match state
        .config
        .domains
        .get(&host)
        .and_then(|d| d.cors.as_ref())
    {
        Some(config) => config,
        None => return next.run(req).await, // No CORS config, pass through.
    };

    // --- MODIFICATION START ---
    // Extract the Origin header into an owned String to resolve the borrow checker error.
    // By cloning the header value, the `origin` variable no longer borrows from `req`.
    let origin = match req
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok().map(String::from))
    {
        Some(origin) => origin,
        None => return next.run(req).await, // Not a CORS request, pass through.
    };
    // --- MODIFICATION END ---

    // Check if the origin is allowed, supporting a wildcard "*" origin.
    let allowed_methods_str = cors_config
        .origins
        .get(&origin) // Now we borrow the owned String 'origin'
        .or_else(|| cors_config.origins.get("*"));

    let is_preflight = req.method() == Method::OPTIONS
        && req
            .headers()
            .contains_key(header::ACCESS_CONTROL_REQUEST_METHOD);

    if is_preflight {
        // --- Handle Preflight Request ---
        let mut resp = Response::builder()
            .status(StatusCode::OK) // 200 OK is a common and safe choice
            .body(Body::empty())
            .unwrap();

        if let Some(methods_str) = allowed_methods_str {
            // Origin is allowed, now check the requested method.
            if let Some(req_method_val) = req.headers().get(header::ACCESS_CONTROL_REQUEST_METHOD) {
                let requested_method_allowed = methods_str.trim() == "*"
                    || methods_str.trim().is_empty()
                    || methods_str.split(',').any(|s| {
                        s.trim()
                            .eq_ignore_ascii_case(req_method_val.to_str().unwrap_or(""))
                    });

                if requested_method_allowed {
                    // Origin and Method are allowed. Add success headers.
                    resp.headers_mut().insert(
                        header::ACCESS_CONTROL_ALLOW_ORIGIN,
                        HeaderValue::from_str(&origin).unwrap(),
                    ); // Borrow 'origin' again
                    resp.headers_mut().insert(
                        header::ACCESS_CONTROL_ALLOW_HEADERS,
                        HeaderValue::from_static("*"),
                    ); // Keep it simple and permissive
                    resp.headers_mut().insert(
                        header::ACCESS_CONTROL_ALLOW_METHODS,
                        HeaderValue::from_str(if methods_str.is_empty() {
                            "*"
                        } else {
                            methods_str
                        })
                        .unwrap(),
                    );
                    resp.headers_mut().insert(
                        header::VARY,
                        HeaderValue::from_static(
                            "Origin, Access-Control-Request-Method, Access-Control-Request-Headers",
                        ),
                    );
                }
            }
        }
        // If origin or method is not allowed, we return the empty 200 OK response,
        // which browsers correctly interpret as a rejection.
        return resp;
    } else {
        // --- Handle Actual Request (e.g., GET, POST) ---
        let mut res = next.run(req).await; // Now `req` can be moved without issue.

        // If the origin was on our list, add the allow origin header to the response.
        if allowed_methods_str.is_some() {
            res.headers_mut().insert(
                header::ACCESS_CONTROL_ALLOW_ORIGIN,
                HeaderValue::from_str(&origin).unwrap(),
            ); // Borrow 'origin' again
            res.headers_mut()
                .append(header::VARY, HeaderValue::from_static("Origin"));
        }
        return res;
    }
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
    if req.headers().get(header::HOST).is_none() {
        if let Some(authority) = req.uri().authority().cloned() {
            req.headers_mut()
                .insert(header::HOST, authority.as_str().parse().unwrap());
        }
    }
    next.run(req).await
}
