//! Terminator impls: `WriteHttpResponse` (H1 encoder, chunked vs
//! Content-Length decision) and `ByteTunnel`.
//!
//! See `spec/crates/engine.md` § _Terminator_ and
//! `spec/crates/core.md` § _H1 egress-side framing decision_.
