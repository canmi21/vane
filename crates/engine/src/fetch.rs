//! Built-in `L7Fetch` / `L4Fetch` impls + the factory registry that
//! `FlowGraph::link` consults.
//!
//! See [`spec/crates/engine.md` § _Fetch_](../../../spec/crates/engine.md#fetch).

#[cfg(feature = "acme")]
pub mod acme_challenge;
#[cfg(feature = "cgi")]
pub mod cgi;
pub mod client_cache;
pub mod dns;
pub mod http_proxy;
pub mod http_synthesize;
pub mod l4_forward;
#[cfg(feature = "h3")]
pub mod quic_pool;
pub mod retry;
pub mod upstream;
pub mod websocket_upgrade;
