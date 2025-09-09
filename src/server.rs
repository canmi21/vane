/* src/server.rs */

use crate::config::AppConfig;
use crate::{config, middleware, proxy, setup, state::AppState, tls::PerDomainCertResolver};
use anyhow::Result;
use axum::Router;
use axum_server::tls_rustls::RustlsConfig;
use fancy_log::{LogLevel, log};
use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::client::legacy::connect::HttpConnector;
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use std::{net::SocketAddr, sync::Arc};
use tokio::signal;

/// Configures and runs the HTTP and HTTPS servers.
pub async fn run() -> Result<()> {
    let app_config = match config::load_config() {
        Ok(cfg) => Arc::new(cfg),
        Err(e) => {
            log(
                LogLevel::Error,
                &format!("Failed to load configuration: {}", e),
            );
            std::process::exit(1);
        }
    };

    if app_config.domains.is_empty() {
        return setup::handle_first_run().await;
    }

    let state = build_shared_state(app_config.clone()).await?;
    let http_handle = spawn_http_server(app_config.clone(), state.clone()).await?;
    let https_handle_opt = spawn_https_server(app_config.clone(), state.clone()).await?;

    let graceful = shutdown_signal();

    if let Some(https_handle) = https_handle_opt {
        tokio::select! {
            _ = graceful => log(LogLevel::Info, "Signal received, shutting down."),
            res = http_handle => match res {
                Ok(Ok(())) => log(LogLevel::Info, "HTTP server exited normally."),
                Ok(Err(e)) => log(LogLevel::Error, &format!("HTTP server error: {}", e)),
                Err(join_err) => log(LogLevel::Error, &format!("HTTP server join error: {}", join_err)),
            },
            res = https_handle => match res {
                Ok(Ok(())) => log(LogLevel::Info, "HTTPS server exited normally."),
                Ok(Err(e)) => log(LogLevel::Error, &format!("HTTPS server error: {}", e)),
                Err(join_err) => log(LogLevel::Error, &format!("HTTPS server join error: {}", join_err)),
            },
        }
    } else {
        tokio::select! {
            _ = graceful => log(LogLevel::Info, "Signal received, shutting down."),
            res = http_handle => match res {
                Ok(Ok(())) => log(LogLevel::Info, "HTTP server exited normally."),
                Ok(Err(e)) => log(LogLevel::Error, &format!("HTTP server error: {}", e)),
                Err(join_err) => log(LogLevel::Error, &format!("HTTP server join error: {}", join_err)),
            },
        }
    }

    Ok(())
}

/// Builds the shared AppState, including the robust HTTP client.
async fn build_shared_state(app_config: Arc<AppConfig>) -> Result<Arc<AppState>> {
    let mut root_store = RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let tls_config = ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    let mut http_connector = HttpConnector::new();
    http_connector.enforce_http(false);
    let https_connector = HttpsConnectorBuilder::new()
        .with_tls_config(tls_config)
        .https_or_http()
        .enable_http1()
        .enable_http2()
        .wrap_connector(http_connector);
    let http_client =
        hyper_util::client::legacy::Client::builder(hyper_util::rt::tokio::TokioExecutor::new())
            .build(https_connector);

    Ok(Arc::new(AppState {
        config: app_config,
        http_client,
    }))
}

/// Spawns the HTTPS server task if any TLS domains are configured.
async fn spawn_https_server(
    app_config: Arc<AppConfig>,
    state: Arc<AppState>,
) -> Result<Option<tokio::task::JoinHandle<Result<(), std::io::Error>>>> {
    let has_tls_domains = app_config.domains.values().any(|d| d.https);
    if !has_tls_domains {
        log(
            LogLevel::Info,
            "No HTTPS domains configured, HTTPS server will not be started.",
        );
        return Ok(None);
    }

    // Create our custom certificate resolver.
    let resolver = PerDomainCertResolver::new(app_config.clone());

    // Corrected: The `with_safe_defaults()` method is removed in newer rustls.
    // The builder now starts with safe defaults.
    let mut server_config = ServerConfig::builder()
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(resolver));

    // Add ALPN protocol support to solve the `curl` warning.
    server_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

    let tls_config = RustlsConfig::from_config(Arc::new(server_config));

    let https_addr = SocketAddr::from(([0, 0, 0, 0], app_config.https_port));
    log(
        LogLevel::Info,
        &format!("Vane HTTPS server listening on {}", https_addr),
    );

    let router = Router::new()
        .fallback(proxy::proxy_handler)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::hsts_handler,
        ))
        .with_state(state.clone());

    let handle = tokio::spawn(async move {
        axum_server::bind_rustls(https_addr, tls_config)
            .serve(router.into_make_service())
            .await
    });

    Ok(Some(handle))
}

/// Spawns the HTTP server task.
async fn spawn_http_server(
    app_config: Arc<AppConfig>,
    state: Arc<AppState>,
) -> Result<tokio::task::JoinHandle<Result<(), std::io::Error>>> {
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

/// Listens for OS signals for graceful shutdown.
async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl-C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
