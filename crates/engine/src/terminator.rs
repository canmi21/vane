//! Terminator impls: `WriteHttpResponse` (H1 encoder, chunked vs
//! Content-Length decision) and `ByteTunnel`.
//!
//! See `spec/crates/engine.md` § _Fetch_ and
//! `spec/crates/engine.md` § _Body streaming_.
