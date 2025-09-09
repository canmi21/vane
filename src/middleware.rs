/* src/middleware.rs */

use crate::state::AppState;
use axum::{
    body::Body,
    extract::State,
    http::{Request, Response, StatusCode},
    middleware::Next,
};
use std::sync::Arc;

pub async fn https_redirect(
    State(_state): State<Arc<AppState>>,
    req: Request<Body>,
    next: Next,
) -> Response<Body> {
    let host = req
        .headers()
        .get("host")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    let is_https = req
        .headers()
        .get("x-forwarded-proto")
        .and_then(|h| h.to_str().ok())
        .map_or(false, |s| s.eq_ignore_ascii_case("https"));

    if !is_https {
        // 尝试从 X-Forwarded-Host 或 Host 头构建 URL
        let host_to_use = req
            .headers()
            .get("x-forwarded-host")
            .or_else(|| req.headers().get("host"))
            .and_then(|h| h.to_str().ok())
            .unwrap_or(host);

        let uri = format!(
            "https://{}{}",
            host_to_use,
            req.uri()
                .path_and_query()
                .map(|pq| pq.as_str())
                .unwrap_or("/")
        );
        return Response::builder()
            .status(StatusCode::MOVED_PERMANENTLY)
            .header("Location", uri)
            .body(Body::empty())
            .unwrap();
    }

    next.run(req).await
}
