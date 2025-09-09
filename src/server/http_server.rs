/* src/server/http_server.rs */

use crate::{config::AppConfig, middleware, proxy, state::AppState};
use anyhow::Result;
use axum::Router;
use fancy_log::{LogLevel, log};
use std::{net::SocketAddr, sync::Arc};
use tokio::task::JoinHandle;

/// Spawns the HTTP server task.
pub async fn spawn(
    app_config: Arc<AppConfig>,
    state: Arc<AppState>,
) -> Result<JoinHandle<Result<(), std::io::Error>>> {
    let http_addr = SocketAddr::from(([0, 0, 0, 0], app_config.http_port));

    let router = Router::new()
        .fallback(proxy::proxy_handler)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::http_request_handler,
        ))
        .with_state(state.clone());

    log(
        LogLevel::Info,
        &format!("Vane HTTP server listening on {}", http_addr),
    );

    let handle = tokio::spawn(async move {
        axum_server::bind(http_addr)
            .serve(router.into_make_service())
            .await
    });

    Ok(handle)
}
