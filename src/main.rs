/* src/main.rs */

mod config;
mod error;
mod models;
mod proxy;
mod routing;
mod state;

use crate::state::AppState;
use anyhow::{Context, Result};
use axum::Router;
use dotenvy::dotenv;
use fancy_log::{LogLevel, log, set_log_level};
use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::client::legacy::connect::HttpConnector;
use lazy_motd::lazy_motd;
use rustls::{ClientConfig, RootCertStore};
use std::{env, fs, sync::Arc};
use tokio::net::TcpListener;
use tokio::signal;

const DEFAULT_MAIN_CONFIG: &str = r#"
# Vane configuration file
# Maps incoming hostnames to their specific configuration files.
# Wildcard domains like "*.example.com" can be defined here later.
[domains]
"example.com" = "example.com.toml"
"#;

const DEFAULT_DOMAIN_CONFIG: &str = r#"
# Routing rules for example.com
[[routes]]
# The path to match. "/" matches all paths by default.
path = "/"
# A list of backend targets. The first one is the primary.
# Others can be added for fallback (feature to be implemented).
targets = ["http://127.0.0.1:5174"]
# Set to true to enable WebSocket proxying for this route (for later).
websocket = false
"#;

#[tokio::main]
async fn main() -> Result<()> {
    // --- Initialization ---
    dotenv().ok();
    let level = env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string());
    let log_level = match level.to_lowercase().as_str() {
        "debug" => LogLevel::Debug,
        "warn" => LogLevel::Warn,
        "error" => LogLevel::Error,
        _ => LogLevel::Info,
    };
    set_log_level(log_level);
    lazy_motd!();

    // --- Load Config & Handle First Run ---
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
        handle_first_run().await?;
        return Ok(()); // Exit after creating configs
    }

    // --- Initialize Services (Robust Method) ---

    // Step 1: Create a root certificate store with webpki roots
    let mut root_store = RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    // Step 2: Create a TLS client configuration
    let tls_config = ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();

    // Step 3: Create a standard HTTP connector
    let mut http_connector = HttpConnector::new();
    http_connector.enforce_http(false);

    // Step 4: Build the HttpsConnector by wrapping the HttpConnector with the TLS config
    let https_connector = HttpsConnectorBuilder::new()
        .with_tls_config(tls_config)
        .https_or_http()
        .enable_http1()
        .enable_http2()
        .wrap_connector(http_connector);

    let http_client =
        hyper_util::client::legacy::Client::builder(hyper_util::rt::tokio::TokioExecutor::new())
            .build(https_connector);

    let state = Arc::new(AppState {
        config: app_config.clone(),
        http_client,
    });
    // --- Create HTTP Router ---
    let app = Router::new()
        .fallback(proxy::proxy_handler)
        .with_state(state);

    let bind_addr = format!("0.0.0.0:{}", app_config.http_port);
    let listener = TcpListener::bind(&bind_addr).await?;
    log(
        LogLevel::Info,
        &format!("Vane HTTP server listening on {}", bind_addr),
    );

    // --- Start Server with Graceful Shutdown ---
    axum::serve(listener, app.into_make_service())
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

/// Sets up a handler for Ctrl+C and termination signals for graceful shutdown.
async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
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

    log(LogLevel::Info, "Signal received, shutting down gracefully.");
}

/// Handles the first-run scenario by creating example configuration files.
async fn handle_first_run() -> Result<()> {
    log(
        LogLevel::Warn,
        "No domains configured or config file not found.",
    );
    log(
        LogLevel::Info,
        "For guidance, please visit: https://github.com/canmi21/vane",
    );

    let (config_path, config_dir) =
        config::get_config_paths().context("Could not determine config paths for first run")?;

    if !config_dir.exists() {
        fs::create_dir_all(&config_dir)
            .with_context(|| format!("Failed to create config directory at {:?}", config_dir))?;
    }

    let domain_config_path = config_dir.join("example.com.toml");

    if !config_path.exists() {
        fs::write(&config_path, DEFAULT_MAIN_CONFIG)
            .with_context(|| format!("Failed to write main config at {:?}", config_path))?;
        log(
            LogLevel::Info,
            &format!("Created example config: {:?}", config_path),
        );
    }

    if !domain_config_path.exists() {
        fs::write(&domain_config_path, DEFAULT_DOMAIN_CONFIG).with_context(|| {
            format!("Failed to write domain config at {:?}", domain_config_path)
        })?;
        log(
            LogLevel::Info,
            &format!("Created example domain config: {:?}", domain_config_path),
        );
    }

    log(
        LogLevel::Info,
        "Example configuration files have been created. Please review and edit them, then start Vane again.",
    );
    Ok(())
}
