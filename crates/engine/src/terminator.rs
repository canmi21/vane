//! Terminator impls: `WriteHttpResponse` (H1 encoder, chunked vs
//! Content-Length decision) and `ByteTunnel`.
//!
//! See `spec/architecture/05-terminator.md` § _Terminator_ and
//! `spec/architecture/03-types.md` § _H1 egress-side framing decision_.
//! Feature: S1-23.
