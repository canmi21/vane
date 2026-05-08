//! `tracing` initialization helper + `FlowLogSink` — a
//! `tokio::sync::broadcast` fan-out consumed by the management API's
//! streaming verbs (`tail_flow`, `tail_log`).
//!
//! See `spec/crates/core.md` § _Error type_ and
//! `spec/crates/mgmt.md` § _Streaming verb lifecycle_.
