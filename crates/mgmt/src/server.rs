//! Management server: Unix `SocketListener` accept loop + per-connection
//! line-delimited JSON dispatch to verb handlers.
//!
//! Stage 1 verbs: `compile_dry_run`, `reload`, `get_active_config`,
//! `stats`, `shutdown`, `list_connections`. Streaming verbs
//! (`tail_flow_log`, `tail_log`) + HTTP-over-TCP transport land in S2.
//!
//! See `spec/architecture/10-management.md`. Features: S1-24, S1-25.
