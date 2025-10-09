/* src/main.rs */

use anyhow::{Context, Result};
use dotenvy::dotenv;
use fancy_log::{LogLevel, log, set_log_level};
use lazy_limit::{Duration, RuleConfig, init_rate_limiter};
use lazy_motd::lazy_motd;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;
use std::{env, time};
use tokio::time as tokio_time;

#[tokio::main]
async fn main() -> Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    dotenv().ok();

    if env::var("RUST_LOG").is_err() {
        unsafe {
            env::set_var("RUST_LOG", "info");
        }
    }

    let level = env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string());
    let log_level = match level.to_lowercase().as_str() {
        "debug" => LogLevel::Debug,
        "warn" => LogLevel::Warn,
        "error" => LogLevel::Error,
        _ => LogLevel::Info,
    };
    set_log_level(log_level);

    if let Err(e) = setup::ensure_status_pages_exist() {
        log(
            LogLevel::Error,
            &format!(
                "Failed to setup status pages: {}. Please check permissions.",
                e
            ),
        );
        std::process::exit(1);
    }

    initialize_shield_limiter().await;
    lazy_motd!();

    // --- MODIFIED: Centralized and corrected startup logic ---
    // 1. Load config ONCE.
    let app_config = Arc::new(config::load_config()?);

    // --- FIX: Remove the incorrect assignment line ---
    // app_config.server_header = std::env::var("SERVER").ok(); // THIS LINE IS REMOVED

    // 2. Check for first-run scenario.
    if app_config.domains.is_empty() {
        return setup::handle_first_run().await;
    }

    // 3. If it's a normal run, spawn background task and then the server.
    spawn_cert_refresh_task(app_config.clone());

    if let Err(e) = server::run(app_config).await {
        log(
            LogLevel::Error,
            &format!("Server exited with an error: {}", e),
        );
        std::process::exit(1);
    }

    Ok(())
}
