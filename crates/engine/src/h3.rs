//! H3 server integration. Body adaptation lives in the standalone
//! [`h3-body`] crate; this module owns only the listener path —
//! QUIC accept loop and per-listener `quinn::Endpoint` virtual socket.
//!
//! See [`spec/crates/engine.md` § _Listeners_](../../../spec/crates/engine.md#listeners).
//!
//! [`h3-body`]: https://docs.rs/h3-body

pub mod listener;
