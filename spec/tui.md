# TUI

The TUI (`vane tui`, built on `ratatui` + `crossterm`) is the only outstanding green-field surface. The CLI scaffold and the binary entry-point exist (`crates/cli/src/tui.rs`); the full view set, key bindings, and update loop are still to be written.

This document locks the capability boundary, view set, update model, and connection surface. Concrete refresh rates, key bindings, color usage, and view-state details are decided during implementation, not here.

## What it is

A management client. Same protocol, same auth, no privileged in-process pathway. The TUI links the same `vane-mgmt` client crate the CLI uses, so transport handling is identical down to retry and reconnection behavior.

## Capability boundary

Read + safe-write. Read verbs and idempotent / non-destructive write verbs are surfaced as TUI actions; destructive or daemon-lifecycle verbs are CLI-only.

| Verb              | TUI action?       | Confirmation prompt            |
| ----------------- | ----------------- | ------------------------------ |
| `compile_dry_run` | yes               | no                             |
| `reload`          | yes               | no (idempotent)                |
| `get_config`      | yes               | no                             |
| `get_connections` | yes               | no                             |
| `get_metrics`     | yes               | no                             |
| `get_pools`       | yes               | no                             |
| `get_upstreams`   | yes               | no                             |
| `get_certs`       | yes               | no                             |
| `tail_flow`       | yes               | no                             |
| `tail_log`        | yes               | no                             |
| `force_renew`     | yes               | yes (may hit ACME rate limits) |
| `pool_drain`      | yes               | yes (forces upstream rotation) |
| `stats`           | yes               | no                             |
| `shutdown`        | **no — CLI only** | —                              |

`shutdown` is CLI-only deliberately: TUI sessions are interactive and prone to misclick; a misclicked shutdown drops every live connection. Operators who want to shut down do so deliberately at a shell.

## View set

Six views ship in the initial TUI; a seventh tracks pool data:

| View            | Data source                          | Notes                                                         |
| --------------- | ------------------------------------ | ------------------------------------------------------------- |
| Connections     | `get_connections` (poll)             | Live table — remote / local / transport / age / current node. |
| Flow log        | `tail_flow` (stream)                 | Stream of predicate / terminator events.                      |
| Structured log  | `tail_log` (stream)                  | Stream of `tracing` events.                                   |
| Certs           | `get_certs` (poll)                   | One row per cert with status / SAN / expiry / next attempt.   |
| Metrics summary | `get_metrics` (poll)                 | Curated subset — error rate, latency p50/p95/p99, pool use.   |
| Config          | `get_config` + `stats` (poll)        | FlowGraph hash, rule count, last reload time, daemon uptime.  |
| Pools           | `get_pools` + `get_upstreams` (poll) | WASM and CGI pool occupancy plus cached upstream entries.     |

## Update model

Mixed: existing streaming verbs continue to stream; the rest are polled. The TUI does not introduce new streaming verbs.

- Streaming-fed views (Flow log, Structured log) consume the verb's event stream directly.
- Poll-fed views issue their `get_*` call on a per-view interval.

Per-view intervals are an implementation choice, not a configuration knob exposed to operators.

## No new mgmt verbs

The TUI consumes the existing verb set. Initial-screen render issues several `get_*` calls in parallel — over Unix socket this is microsecond-scale and not worth optimizing with a bulk `subscribe_state` verb.

```rust
// TODO(tui-bulk-subscribe): if profiling later shows the per-view-poll
// pattern matters, a bulk subscribe verb is additive — not a breaking
// change to existing TUI code.
```

## Connection surface

Both management transports, mirroring the CLI:

```
vane tui --socket /var/run/vaned.sock                      Unix socket
vane tui --http https://admin.example.com --token-env Y    HTTP-over-TCP
```

Argument shapes follow the CLI subcommand layout in [`crates/cli.md`](crates/cli.md).

## What's testable, what's not

The TUI breaks at the rendering / input boundary:

- **Sub-agent automated** — the pure layer beneath the UI:
  - Data adapters: `FlowLogEvent` → `FlowLogRow`, `StatsSnapshot` → `OverviewModel`.
  - View state machine: `(state, input_event) → new_state` is pure; test with a fixed input trace.
  - Input mapping tables: `KeyEvent` → `Action`.
- **Interactively verified by the user** — ratatui widget rendering, crossterm side effects.

No ratatui rendering is tested. No crossterm side-effects are tested. The split keeps tests deterministic; the rendering layer is verified by eye.

## Deferred to implementation

Decided during implementation, not in this spec:

- Per-view refresh rates.
- Key bindings (navigation, command mode, view switching).
- Color usage (status highlighting, warnings, severity).
- View layout (panes, status bar, command line).
- Behavior on transport disconnect / daemon restart.

The implementing PR's notes record these. They are not load-bearing architectural decisions; they are UI calibration.
