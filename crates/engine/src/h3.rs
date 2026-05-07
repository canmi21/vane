//! H3 server + body integration. `body` adapts `h3`'s split
//! `recv_data` / `recv_trailers` API to `http_body::Body`; `listener`
//! drives QUIC accept loops and the per-listener `quinn::Endpoint`
//! virtual socket.
//!
//! See [`spec/crates/engine.md` § _Listeners_](../../../spec/crates/engine.md#listeners).

pub mod body;
pub mod listener;
