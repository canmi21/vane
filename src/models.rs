/* src/models.rs */

use serde::Deserialize;
use std::collections::HashMap;

// MODIFIED: Represents the new, more detailed CORS configuration.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct CorsConfig {
    // A map where the key is the origin (e.g., "https://app.example.com")
    // and the value is a comma-separated string of allowed methods ("GET, POST").
    // An empty string or "*" in the value means all methods are allowed for that origin.
    #[serde(default)]
    pub origins: HashMap<String, String>,
}

// NEW: Represents the method filtering configuration for an entire domain.
#[derive(Debug, Deserialize, Clone)]
pub struct MethodsConfig {
    // A comma-separated string of allowed HTTP methods (e.g., "GET, POST, OPTIONS"), or "*" for all.
    pub allow: String,
}

/// Defines the behavior for plain HTTP requests.
#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum HttpOptions {
    Upgrade,
    Reject,
    Allow,
}

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
}

/// Represents the top-level structure of the main `config.toml`.
#[derive(Debug, Deserialize, Clone)]
pub struct MainConfig {
    #[serde(default)]
    pub domains: HashMap<String, String>,
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

/// Represents a rate limit rule (period and number of requests).
#[derive(Debug, Deserialize, Clone)]
pub struct RateLimitRule {
    pub period: String,
    pub requests: u32,
}

/// Represents a rate limit rule associated with a specific path.
#[derive(Debug, Deserialize, Clone)]
pub struct RateLimitRouteRule {
    pub path: String,
    #[serde(flatten)]
    pub rule: RateLimitRule,
}

/// Represents the rate limiting configuration for a domain.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct RateLimitConfig {
    pub default: Option<RateLimitRule>,
    #[serde(default)]
    pub routes: Vec<RateLimitRouteRule>,
    #[serde(default)]
    pub overrides: Vec<RateLimitRouteRule>,
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
    #[serde(default)]
    pub http3: bool,
    pub tls: Option<TlsConfig>,
    #[serde(default)]
    pub routes: Vec<Route>,
    #[serde(default)]
    pub rate_limit: RateLimitConfig,

    // MODIFIED: Use the new CorsConfig struct.
    pub cors: Option<CorsConfig>,

    // NEW: Add the optional method filtering configuration.
    pub methods: Option<MethodsConfig>,
}
