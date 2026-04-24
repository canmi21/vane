//! L1 security floor — daemon self-preservation: per-IP + global
//! connection caps, header / body timeouts, handshake-rate caps, plus
//! the compile-time floor enforcement for `VANE_SEC_*` env vars.
//!
//! State is daemon-scoped (lives outside `FlowGraph`), so config reload
//! does not reset counters. See `spec/architecture/13-rate-limit.md` §
//! _L1 — Daemon self-preservation_. Feature: S1-30.
