//! `protocol_detect` L4 middleware: peek ≤ 8 KiB, HTTP/1.x prefix +
//! H2 preface. (TLS `ClientHello` + QUIC Initial land in Stage 2/3.)
//!
//! See `spec/architecture/06-l4.md` § _Protocol detection_.
//! Feature: S1-16.
