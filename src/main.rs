/* src/main.rs */

mod config;
mod error;
mod middleware;
mod models;
mod proxy;
mod ratelimit;
mod routing;
mod server;
mod setup;
mod state;
mod tls;

use anyhow::Result;
use dotenvy::dotenv;
use fancy_log::{LogLevel, log, set_log_level};
// Import lazy-limit for the mandatory shield.
use lazy_limit::{Duration, RuleConfig, init_rate_limiter};
use lazy_motd::lazy_motd;
use std::env;

/// Initializes the mandatory global rate limit shield.
async fn initialize_shield_limiter() {
    log(
        LogLevel::Info,
        "Initializing mandatory shield rate limit: 30 requests/second.",
    );
    init_rate_limiter!(
        default: RuleConfig::new(Duration::seconds(1), 30),
        max_memory: Some(64 * 1024 * 1024) // 64MB for the shield
    )
    .await;
}

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
    tracing_subscriber::fmt::init();

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

    // Initialize the hard-coded lazy-limit shield first.
    initialize_shield_limiter().await;

    lazy_motd!();

    // Load config and start servers inside server::run.
    // The run function no longer takes arguments.
    if let Err(e) = server::run().await {
        log(
            LogLevel::Error,
            &format!("Server exited with an error: {}", e),
        );
        std::process::exit(1);
    }

    Ok(())
}
