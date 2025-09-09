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

    // Set up logging
    let level = env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string());
    let log_level = match level.to_lowercase().as_str() {
        "debug" => LogLevel::Debug,
        "warn" => LogLevel::Warn,
        "error" => LogLevel::Error,
        _ => LogLevel::Info,
    };
    set_log_level(log_level);

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
