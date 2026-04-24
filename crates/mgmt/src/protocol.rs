//! Management wire format: `Request` / `Response` / `Stream` frame
//! shapes, shared across both transports. Stage 1 ships the
//! line-delimited JSON form over Unix; NDJSON-over-chunked-HTTP lands in
//! Stage 2.
//!
//! See `spec/architecture/10-management.md`. Feature: S1-24.
