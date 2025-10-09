/* src/models.rs */

use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize, Clone, Default)]
pub struct CorsConfig {
    #[serde(default)]
    pub origins: HashMap<String, String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct MethodsConfig {
    pub allow: String,
}

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

#[derive(Debug, Deserialize, Clone)]
pub struct Route {
    #[serde(default = "default_path")]
    pub path: String,
    pub targets: Vec<String>,
    #[serde(default)]
    pub websocket: bool,
}

fn default_path() -> String {
    "/".to_string()
}

#[derive(Debug, Deserialize, Clone)]
pub struct RateLimitRule {
    pub period: String,
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
    pub cors: Option<CorsConfig>,
    pub methods: Option<MethodsConfig>,
}
