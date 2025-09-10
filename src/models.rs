/* src/models.rs */

use serde::Deserialize;
use std::collections::HashMap;

// --- Existing Enums and Structs ---

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

#[derive(Debug, Deserialize, Clone)]
pub struct TlsConfig {
    pub cert: String,
    pub key: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct MainConfig {
    #[serde(default)]
    pub domains: HashMap<String, String>,
}

// --- New Structs for Rate Limiting ---

#[derive(Debug, Deserialize, Clone)]
pub struct RateLimitRule {
    pub period: String, // e.g., "1s", "10m", "1h"
    pub requests: u32,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RateLimitRouteRule {
    pub path: String,
    #[serde(flatten)]
    pub rule: RateLimitRule,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct RateLimitConfig {
    pub default: Option<RateLimitRule>,
    #[serde(default)]
    pub routes: Vec<RateLimitRouteRule>,
    #[serde(default)]
    pub overrides: Vec<RateLimitRouteRule>,
}

// --- Updated DomainConfig ---

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

    // New field for rate limiting configuration
    #[serde(default)]
    pub rate_limit: RateLimitConfig,
}

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
