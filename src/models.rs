/* src/models.rs */

use serde::Deserialize;
use std::collections::HashMap;

/// Defines the behavior for plain HTTP requests.
#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum HttpOptions {
    Upgrade,
    Reject,
    Allow,
}

/// Provides a default value for HttpOptions.
impl Default for HttpOptions {
    fn default() -> Self {
        HttpOptions::Allow
    }
}

/// Represents TLS settings for a domain.
#[derive(Debug, Deserialize, Clone)]
pub struct TlsConfig {
    pub cert: String,
    pub key: String,
    // Note: min_version has been removed and is now a global setting in the server
    // for compatibility with the latest rustls API design.
}

/// Represents the top-level structure of the main `config.toml`.
/// It now only maps a hostname to its configuration file.
#[derive(Debug, Deserialize, Clone)]
pub struct MainConfig {
    #[serde(default)]
    pub domains: HashMap<String, String>,
}

/// Represents the configuration for a specific domain (e.g., `example.com.toml`).
#[derive(Debug, Deserialize, Clone)]
pub struct DomainConfig {
    #[serde(default)]
    pub https: bool,

    #[serde(default)]
    pub http_options: HttpOptions,

    #[serde(default)]
    pub hsts: bool,

    pub tls: Option<TlsConfig>,

    #[serde(default)]
    pub routes: Vec<Route>,
}

/// Represents a single routing rule within a domain's configuration.
#[derive(Debug, Deserialize, Clone)]
pub struct Route {
    #[serde(default = "default_path")]
    pub path: String,
    pub targets: Vec<String>,
    #[allow(dead_code)]
    #[serde(default)]
    pub websocket: bool,
}

fn default_path() -> String {
    "/".to_string()
}
