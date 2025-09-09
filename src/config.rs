/* src/config.rs */

use crate::models::{DomainConfig, MainConfig};
use anyhow::{Context, Result};
use fancy_log::{LogLevel, log};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use toml;

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub http_port: u16,
    pub https_port: u16,
    pub domains: HashMap<String, DomainConfig>,
}

/// Returns the main config file path and its parent directory.
pub fn get_config_paths() -> Result<(PathBuf, PathBuf)> {
    let config_path_str = env::var("CONFIG").unwrap_or_else(|_| "~/vane/config.toml".to_string());
    let config_path = PathBuf::from(shellexpand::tilde(&config_path_str).into_owned());
    let config_dir = config_path
        .parent()
        .map(PathBuf::from)
        .context("Could not determine config directory")?;
    Ok((config_path, config_dir))
}

/// Loads all configurations from the environment and TOML files.
pub fn load_config() -> Result<AppConfig> {
    let http_port = env::var("BIND_HTTP_PORT")
        .unwrap_or_else(|_| "80".to_string())
        .parse::<u16>()
        .context("Invalid BIND_HTTP_PORT")?;

    let https_port = env::var("BIND_HTTPS_PORT")
        .unwrap_or_else(|_| "443".to_string())
        .parse::<u16>()
        .context("Invalid BIND_HTTPS_PORT")?;

    let (config_path, config_dir) = get_config_paths()?;

    log(
        LogLevel::Info,
        &format!("Loading main config from {:?}", config_path),
    );

    if !config_path.exists() {
        return Ok(AppConfig {
            http_port,
            https_port,
            domains: HashMap::new(),
        });
    }

    let main_config_content = fs::read_to_string(&config_path)
        .with_context(|| format!("Failed to read main config file at {:?}", config_path))?;
    let main_config: MainConfig =
        toml::from_str(&main_config_content).context("Failed to parse main config file")?;

    let mut domains = HashMap::new();
    for (hostname, file_path_str) in main_config.domains {
        let domain_config_path = config_dir.join(&file_path_str);
        log(
            LogLevel::Debug,
            &format!(
                "Loading domain config for '{}' from {:?}",
                hostname, domain_config_path
            ),
        );

        let domain_config_content = fs::read_to_string(&domain_config_path)
            .with_context(|| format!("Failed to read domain config for '{}'", hostname))?;
        let domain_config: DomainConfig = toml::from_str(&domain_config_content)
            .with_context(|| format!("Failed to parse domain config for '{}'", hostname))?;

        // Validate that if https is true, tls config exists.
        if domain_config.https && domain_config.tls.is_none() {
            return Err(anyhow::anyhow!(
                "Domain '{}' has https=true but no [tls] configuration.",
                hostname
            ));
        }

        domains.insert(hostname, domain_config);
    }

    Ok(AppConfig {
        http_port,
        https_port,
        domains,
    })
}
