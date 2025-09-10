/* src/state.rs */

use crate::config::AppConfig;
use governor::{RateLimiter, clock::DefaultClock, state::keyed::DefaultKeyedStateStore};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;

// This type alias remains the same. It represents ONE keyed rate limiter.
pub type ConfigurableRateLimiter =
    RateLimiter<IpAddr, DefaultKeyedStateStore<IpAddr>, DefaultClock>;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<AppConfig>,
    pub http_client: hyper_util::client::legacy::Client<
        hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>,
        axum::body::Body,
    >,
    // This limiter is for the global 'default' rule.
    pub configurable_limiter: Arc<ConfigurableRateLimiter>,
    // FIX: Add HashMaps to store pre-built limiters for each specific route.
    // We store them in Arcs to allow shared ownership.
    pub route_limiters: Arc<HashMap<String, Arc<ConfigurableRateLimiter>>>,
    pub override_limiters: Arc<HashMap<String, Arc<ConfigurableRateLimiter>>>,
}
