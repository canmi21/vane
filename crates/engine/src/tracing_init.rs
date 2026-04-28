//! `tracing` initialization helper + `FlowLogSink` — a
//! `tokio::sync::broadcast` fan-out consumed by the management API's
//! streaming verbs (`tail_flow`, `tail_log`).
//!
//! See `spec/architecture/17-error-type.md` § _Flow log error events_ and
//! `spec/architecture/10-management.md` § _Streaming verb lifecycle_.
//! Feature: S1-29.
