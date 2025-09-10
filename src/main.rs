/* src/main.rs */

mod config;
mod error;
mod middleware;
mod models;
mod proxy;
mod routing;
mod server;
mod setup;
mod state;
mod tls;

use anyhow::Result;
use dotenvy::dotenv;
use fancy_log::{LogLevel, log, set_log_level};
use lazy_motd::lazy_motd;
use std::env;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize the rustls crypto provider
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    // Initialize environment variables from .env file
    dotenv().ok();

    // Set up logging. We use `tracing` now for better library support.
    if env::var("RUST_LOG").is_err() {
        // Correctly handle the unsafe call to `set_var`.
        // This is safe here as it's called before any threads are spawned.
        unsafe {
            env::set_var("RUST_LOG", "info");
        }
    }
    tracing_subscriber::fmt::init();

    // Setup for fancy_log (can be used alongside tracing)
    let level = env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string());
    let log_level = match level.to_lowercase().as_str() {
        "debug" => LogLevel::Debug,
        "warn" => LogLevel::Warn,
        "error" => LogLevel::Error,
        _ => LogLevel::Info,
    };
    set_log_level(log_level);

    // Ensure the default status pages exist in the config directory.
    // This will create them on first run or if the user deletes them.
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

    // Display the startup message
    lazy_motd!();

    // Delegate all server logic to the server module
    if let Err(e) = server::run().await {
        log(
            LogLevel::Error,
            &format!("Server exited with an error: {}", e),
        );
        std::process::exit(1);
    }

    Ok(())
}
