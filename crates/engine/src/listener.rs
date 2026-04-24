//! TCP accept loop + backoff-bind + `SO_REUSEADDR` + cancellation + soft
//! drain, plus dual-stack IPv4/IPv6 expansion driven by `VANE_BIND_IPV{4,6}`.
//!
//! See `spec/architecture/01-topology.md`, `spec/architecture/06-l4.md`,
//! `spec/architecture/09-config.md` § _`ListenSpec` grammar_.
//! Features: S1-13, S1-14.
