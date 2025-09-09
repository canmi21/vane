/* src/middleware.rs */

use crate::state::AppState;
use axum::{
    body::Body,
    extract::State,
    http::{Request, Uri},
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::extract::Host;
use std::sync::Arc;

/// An Axum middleware that redirects HTTP requests to HTTPS if the domain is configured for TLS.
pub async fn https_redirect(
    State(state): State<Arc<AppState>>,
    Host(host): Host,
    req: Request<Body>,
    next: Next,
) -> Response {
    if let Some((_, Some(_))) = state.config.domains.get(&host) {
        let mut parts = req.uri().clone().into_parts();
        parts.scheme = Some("https".try_into().unwrap());

        let https_uri = Uri::from_parts(parts).unwrap().to_string();

        return Redirect::permanent(&https_uri).into_response();
    }

    next.run(req).await
}
