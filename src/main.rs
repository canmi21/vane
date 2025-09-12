/* src/main.rs */

mod acme_client;
mod config;
mod error;
mod middleware;
mod models;
mod path_matcher;
mod proxy;
mod ratelimit;
mod routing;
mod server;
mod setup;
mod state;
mod tls;

use anyhow::{Context, Result};
use config::AppConfig;
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

/// Spawns a background task to periodically refresh ACME certificates.
fn spawn_cert_refresh_task(app_config: Arc<AppConfig>) {
    // Only spawn the task if a certificate server is configured.
    if app_config.cert_server.is_none() {
        return;
    }

    tokio::spawn(async move {
        log(
            LogLevel::Info,
            "Spawning background task for certificate renewal.",
        );
        let timestamp_file = app_config.cert_dir.join("timestamp");
        let mut interval = tokio_time::interval(tokio_time::Duration::from_secs(3600)); // Check every hour

        loop {
            interval.tick().await;
            log(
                LogLevel::Debug,
                "Performing hourly check for certificate renewal.",
            );

            let should_refresh = match fs::metadata(&timestamp_file) {
                Ok(metadata) => {
                    let modified_time = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                    let elapsed = modified_time.elapsed().unwrap_or_default();
                    elapsed.as_secs() > 86400 // Refresh if older than 24 hours (24 * 60 * 60)
                }
                Err(_) => true, // File doesn't exist, so we should refresh.
            };

            if should_refresh {
                log(
                    LogLevel::Info,
                    "Certificate refresh needed. Starting renewal process...",
                );
                match refresh_all_certificates(&app_config).await {
                    Ok(true) => {
                        log(
                            LogLevel::Info,
                            "All certificates renewed successfully. Vane will now restart to apply changes.",
                        );
                        // Write the timestamp file on success
                        let _ = fs::write(
                            &timestamp_file,
                            SystemTime::now()
                                .duration_since(time::UNIX_EPOCH)
                                .unwrap()
                                .as_secs()
                                .to_string(),
                        );
                        // Gracefully exit so the service manager can restart us with the new certs.
                        std::process::exit(0);
                    }
                    Ok(false) => {
                        log(
                            LogLevel::Info,
                            "No certificates required renewal. Check completed.",
                        );
                        // Write timestamp file anyway to prevent re-checking for 24 hours
                        let _ = fs::write(
                            &timestamp_file,
                            SystemTime::now()
                                .duration_since(time::UNIX_EPOCH)
                                .unwrap()
                                .as_secs()
                                .to_string(),
                        );
                    }
                    Err(e) => {
                        log(
                            LogLevel::Error,
                            &format!("Certificate renewal process failed: {}", e),
                        );
                        // We don't write the timestamp on failure, so we'll try again in an hour.
                    }
                }
            }
        }
    });
}

/// The main certificate renewal logic. Returns Ok(true) if certs were actually refreshed.
async fn refresh_all_certificates(app_config: &Arc<AppConfig>) -> Result<bool> {
    let server_url = app_config.cert_server.as_ref().unwrap();
    let tmp_dir = app_config.cert_dir.join("tmp");

    let hosts_to_refresh: Vec<_> = app_config
        .domains
        .iter()
        .filter(|(_, dc)| dc.https && dc.tls.is_some())
        .map(|(host, _)| host.clone())
        .collect();

    if hosts_to_refresh.is_empty() {
        log(
            LogLevel::Info,
            "No HTTPS domains configured, skipping refresh.",
        );
        return Ok(false);
    }

    fs::create_dir_all(&tmp_dir).context("Failed to create temporary cert directory")?;

    for host in &hosts_to_refresh {
        let (cert_path, key_path) = get_cert_paths_for_host(&tmp_dir, host);
        acme_client::fetch_and_save_certificate(host, server_url, &cert_path, &key_path).await?;
    }

    log(
        LogLevel::Info,
        "All certificates fetched. Moving to final destination.",
    );
    for host in &hosts_to_refresh {
        let (tmp_cert_path, tmp_key_path) = get_cert_paths_for_host(&tmp_dir, host);
        let (final_cert_path, final_key_path) = get_cert_paths_for_host(&app_config.cert_dir, host);
        fs::rename(tmp_cert_path, final_cert_path)?;
        fs::rename(tmp_key_path, final_key_path)?;
    }

    fs::remove_dir_all(tmp_dir)?;

    Ok(true)
}

fn get_cert_paths_for_host(base_dir: &Path, host: &str) -> (PathBuf, PathBuf) {
    let domain_config_path = base_dir.to_path_buf();
    let cert_path = domain_config_path.join(format!("{}.pem", host));
    let key_path = domain_config_path.join(format!("{}.key", host));
    (cert_path, key_path)
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
