/* src/server/mod.rs */

mod http3_server;
mod http_server;
mod https_server;

use crate::{config, setup, state::AppState};
use anyhow::Result;
use fancy_log::{LogLevel, log, set_log_level};
use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::client::legacy::connect::HttpConnector;
use rustls::{ClientConfig, RootCertStore};
use std::sync::Arc;
use tokio::signal;

/// Configures and runs all servers (HTTP, HTTPS/TCP, HTTPS/UDP).
pub async fn run() -> Result<()> {
    set_log_level(LogLevel::Debug);

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

    // Spawn all servers
    let http_handle = http_server::spawn(app_config.clone(), state.clone()).await?;
    let https_handle_opt = https_server::spawn(app_config.clone(), state.clone()).await?;
    let http3_handle_opt = http3_server::spawn(app_config.clone(), state.clone()).await?;

    let graceful = shutdown_signal();
    tokio::pin!(graceful);

    match (https_handle_opt, http3_handle_opt) {
        (Some(https), Some(h3)) => tokio::select! {
            _ = &mut graceful => log(LogLevel::Info, "Signal received, shutting down."),
            res = http_handle => handle_task_result("HTTP", res),
            res = https => handle_task_result("HTTPS/TCP", res),
            res = h3 => handle_task_result("HTTPS/UDP (HTTP/3)", res),
        },
        (Some(https), None) => tokio::select! {
            _ = &mut graceful => log(LogLevel::Info, "Signal received, shutting down."),
            res = http_handle => handle_task_result("HTTP", res),
            res = https => handle_task_result("HTTPS/TCP", res),
        },
        _ => tokio::select! {
            _ = &mut graceful => log(LogLevel::Info, "Signal received, shutting down."),
            res = http_handle => handle_task_result("HTTP", res),
        },
    }

    Ok(())
}

/// Helper to log the exit status of a server task.
fn handle_task_result(
    server_name: &str,
    res: Result<Result<(), impl std::fmt::Display + Send + Sync>, tokio::task::JoinError>,
) {
    match res {
        Ok(Ok(())) => log(
            LogLevel::Info,
            &format!("{} server exited normally.", server_name),
        ),
        Ok(Err(e)) => log(
            LogLevel::Error,
            &format!("{} server error: {}", server_name, e),
        ),
        Err(join_err) => log(
            LogLevel::Error,
            &format!("{} server join error: {}", server_name, join_err),
        ),
    }
}

/// Builds the shared AppState.
async fn build_shared_state(app_config: Arc<config::AppConfig>) -> Result<Arc<AppState>> {
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
