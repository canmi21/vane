//! Built-in `L7Fetch` / `L4Fetch` impls + the factory registry that
//! `FlowGraph::link` consults.
//!
//! See `spec/architecture/05-terminator.md`, `spec/architecture/07-l7.md`.
//! Features: S1-18 (`L4ForwardFetch`), S1-19 (`HttpProxyFetch`, H1→H1),
//! S1-20 (`HttpSynthesizeFetch`).

pub mod http_proxy;
pub mod http_synthesize;
pub mod l4_forward;
pub mod upstream;
pub mod websocket_upgrade;
