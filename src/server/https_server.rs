/* src/server/https_server.rs */

use crate::{config::AppConfig, middleware, proxy, state::AppState, tls::PerDomainCertResolver};
use anyhow::Result;
use axum::{Router, middleware as axum_middleware};
use axum_server::tls_rustls::RustlsConfig;
use fancy_log::{LogLevel, log};
use rustls::ServerConfig;
use std::{net::SocketAddr, sync::Arc};
use tokio::task::JoinHandle;

/// Spawns the HTTPS/TCP (HTTP/1.1, HTTP/2) server task.
pub async fn spawn(
    app_config: Arc<AppConfig>,
    state: Arc<AppState>,
) -> Result<Option<JoinHandle<Result<(), std::io::Error>>>> {
    if !app_config.domains.values().any(|d| d.https) {
        return Ok(None);
    }

    let resolver = PerDomainCertResolver::new(app_config.clone());
    let mut server_config = ServerConfig::builder()
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(resolver));
    server_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

    let tls_config = RustlsConfig::from_config(Arc::new(server_config));
    let https_addr = SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 0], app_config.https_port));

    log(
        LogLevel::Info,
        &format!(
            "Vane HTTPS (H2, H1.1) server listening on [::]:TCP:{}",
            app_config.https_port
        ),
    );

    let router = Router::new()
        .fallback(proxy::proxy_handler)
        // Inject host header first, as other middleware depends on it.
        .layer(axum_middleware::from_fn(middleware::inject_host_header))
        // NEW: Add method filtering after host injection.
        .layer(axum_middleware::from_fn_with_state(
            state.clone(),
            middleware::method_filter_handler,
        ))
        // Add the CORS layer near the outside.
        .layer(axum_middleware::from_fn_with_state(
            state.clone(),
            middleware::cors_handler,
        ))
        .layer(axum_middleware::from_fn_with_state(
            state.clone(),
            middleware::rate_limit_handler,
        ))
        .layer(axum_middleware::from_fn_with_state(
            state.clone(),
            middleware::alt_svc_handler,
        ))
        .layer(axum_middleware::from_fn_with_state(
            state.clone(),
            middleware::hsts_handler,
        ))
        .with_state(state.clone());

    let handle = tokio::spawn(async move {
        axum_server::bind_rustls(https_addr, tls_config)
            .serve(router.into_make_service_with_connect_info::<SocketAddr>())
            .await
    });

    Ok(Some(handle))
}
