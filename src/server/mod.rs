/* src/server/mod.rs */

mod http3_server;
mod http_server;
mod https_server;

use crate::{
    config::{self, AppConfig},
    setup,
    state::{AppState, ConfigurableRateLimiter},
};
use anyhow::{Context, Result};
use fancy_log::{LogLevel, log};
use governor::{Quota, RateLimiter};
use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::client::legacy::connect::HttpConnector;
use rustls::{ClientConfig, RootCertStore};
use std::collections::HashMap;
use std::num::NonZeroU32;
use std::{sync::Arc, time::Duration};
use tokio::signal;

/// Parses a string like "1s", "10m" into a std::time::Duration for Governor.
fn parse_std_duration(period: &str) -> Result<Duration> {
    let period = period.to_lowercase();
    let value_str = period.trim_end_matches(|c: char| !c.is_numeric());
    let unit = period.trim_start_matches(|c: char| c.is_numeric());

    let value = value_str.parse::<u64>().context("Invalid duration value")?;

    match unit {
        "s" => Ok(Duration::from_secs(value)),
        "m" => Ok(Duration::from_secs(value * 60)),
        "h" => Ok(Duration::from_secs(value * 60 * 60)),
        _ => Err(anyhow::anyhow!("Unsupported duration unit: {}", unit)),
    }
}

/// Builds the user-configurable global rate limiter.
fn build_global_limiter(config: &AppConfig) -> Result<Arc<ConfigurableRateLimiter>> {
    // To represent "infinite", we set a very high burst size.
    let mut default_quota = Quota::per_second(NonZeroU32::new(u32::MAX).unwrap());
    let mut rule_found = false;

    for (hostname, domain_config) in &config.domains {
        if let Some(ref rule) = domain_config.rate_limit.default {
            let period = parse_std_duration(&rule.period)?;
            if let Some(requests_burst) = NonZeroU32::new(rule.requests) {
                if !period.is_zero() {
                    default_quota = Quota::with_period(period)
                        .context("Invalid period for quota")?
                        .allow_burst(requests_burst);
                    log(
                        LogLevel::Info,
                        &format!(
                            "Configurable global rate limit from '{}': {} req/{}",
                            hostname, rule.requests, rule.period
                        ),
                    );
                    rule_found = true;
                    break;
                }
            }
        }
    }

    if !rule_found {
        log(
            LogLevel::Info,
            "No user-configurable global rate limit found. Defaulting to unlimited.",
        );
    }
    Ok(Arc::new(RateLimiter::keyed(default_quota)))
}

/// Builds all route-specific rate limiters and stores them in HashMaps.
fn build_route_limiters(
    config: &AppConfig,
) -> Result<(
    Arc<HashMap<String, Arc<ConfigurableRateLimiter>>>,
    Arc<HashMap<String, Arc<ConfigurableRateLimiter>>>,
)> {
    let mut route_limiters = HashMap::new();
    let mut override_limiters = HashMap::new();

    for (hostname, domain_config) in &config.domains {
        // Build limiters for normal routes
        for route_rule in &domain_config.rate_limit.routes {
            if let (Ok(period), Some(requests_burst)) = (
                parse_std_duration(&route_rule.rule.period),
                NonZeroU32::new(route_rule.rule.requests),
            ) {
                if !period.is_zero() {
                    if let Some(quota) = Quota::with_period(period) {
                        let limiter =
                            Arc::new(RateLimiter::keyed(quota.allow_burst(requests_burst)));
                        let key = format!("{}{}", hostname, route_rule.path);
                        log(
                            LogLevel::Debug,
                            &format!(
                                "Created route limiter for '{}': {} req/{}",
                                key, route_rule.rule.requests, route_rule.rule.period
                            ),
                        );
                        route_limiters.insert(key, limiter);
                    }
                }
            }
        }
        // Build limiters for override routes
        for override_rule in &domain_config.rate_limit.overrides {
            if let (Ok(period), Some(requests_burst)) = (
                parse_std_duration(&override_rule.rule.period),
                NonZeroU32::new(override_rule.rule.requests),
            ) {
                if !period.is_zero() {
                    if let Some(quota) = Quota::with_period(period) {
                        let limiter =
                            Arc::new(RateLimiter::keyed(quota.allow_burst(requests_burst)));
                        let key = format!("{}{}", hostname, override_rule.path);
                        log(
                            LogLevel::Debug,
                            &format!(
                                "Created override limiter for '{}': {} req/{}",
                                key, override_rule.rule.requests, override_rule.rule.period
                            ),
                        );
                        override_limiters.insert(key, limiter);
                    }
                }
            }
        }
    }
    Ok((Arc::new(route_limiters), Arc::new(override_limiters)))
}

/// Builds the shared AppState, creating all necessary components including all rate limiters.
async fn build_shared_state(app_config: Arc<config::AppConfig>) -> Result<Arc<AppState>> {
    // Build all limiters at startup.
    let configurable_limiter = build_global_limiter(&app_config)?;
    let (route_limiters, override_limiters) = build_route_limiters(&app_config)?;

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
        configurable_limiter,
        route_limiters,
        override_limiters,
    }))
}

/// Configures and runs all servers (HTTP, HTTPS/TCP, HTTPS/UDP).
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

    // The HTTP server is non-optional. If it fails to spawn, the application cannot continue.
    // We convert the Option<JoinHandle> returned by spawn() into a Result.
    // If the Option is None, .context() creates an error, which is then propagated by the `?` operator.
    // If it's Some(handle), the `?` unwraps the Result, and we get the JoinHandle directly.
    let http_handle = http_server::spawn(app_config.clone(), state.clone())
        .await?
        .context("The primary HTTP server failed to start and is required.")?;

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
