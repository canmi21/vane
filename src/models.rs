/* src/models.rs */

use serde::Deserialize;
use std::collections::HashMap;

/// Represents TLS certificate and private key paths in the config.
#[derive(Debug, Deserialize, Clone)]
pub struct TlsConfig {
    pub cert: String,
    pub key: String,
}

/// Represents an entry for a domain in the main config file.
#[derive(Debug, Deserialize, Clone)]
pub struct MainConfigEntry {
    pub file: String,
    pub tls: Option<TlsConfig>, // TLS config is optional per domain.
}

/// Represents the top-level structure of the main `config.toml`.
#[derive(Debug, Deserialize, Clone)]
pub struct MainConfig {
    #[serde(default)]
    pub domains: HashMap<String, MainConfigEntry>,
}

/// Represents the configuration for a specific domain (e.g., `example.com.toml`).
#[derive(Debug, Deserialize, Clone)]
pub struct DomainConfig {
    #[serde(default)]
    pub routes: Vec<Route>,
}

/// Represents a single routing rule within a domain's configuration.
#[derive(Debug, Deserialize, Clone)]
pub struct Route {
    #[serde(default = "default_path")]
    pub path: String,
    pub targets: Vec<String>,
    #[allow(dead_code)] // Field is planned for future WebSocket support.
    #[serde(default)]
    pub websocket: bool,
}

fn default_path() -> String {
    "/".to_string()
}
