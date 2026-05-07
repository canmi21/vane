# vane (CLI binary)

Source: [`crates/cli/`](../../crates/cli/). Package name: `vane`. Folder: `crates/cli/`.

User-facing terminal binary. Two modes: stateless CLI for short-lived calls, plus a long-lived TUI mode that connects to one running `vaned`. Does not depend on `vane-engine` — CLI / TUI are pure clients of the management protocol.

## Owns

- CLI entry point (`clap` derive), command dispatch. Source: `main.rs`.
- TUI shell. Source: `tui.rs`. See [`../tui.md`](../tui.md).
- Client wiring against `vane-mgmt`.
- `vane compile <DIR>` compiles via `vane-core` (no engine needed; outputs JSON).
- `build.rs` — emits compile-time env vars consumed by `main.rs` via `env!()`.

## Crate dependencies

`vane-core`, `vane-mgmt` + `clap`, `ratatui`, `crossterm`, `tokio`.

This crate must build fast. Deployment footprint is a single statically-linked binary, ~5–10 MiB.

## Subcommand layout

Two design rules:

1. **No hyphens in subcommand names.** Multi-word verbs become nested subcommand groups (`vane get config`), not kebab strings (`vane get-active-config`). Keystrokes stay short, tab completion stays clean.
2. **Flat for global actions, grouped for data / streams.** `ping` / `reload` / `shutdown` / `compile` are top-level — they are themselves verbs. `get` and `tail` are dispatch groups for snapshot / streaming reads.

```
vane --version | -v
vane --help    | -h

# Global actions
vane ping                          liveness check
vane stats                         daemon summary (uptime, graph hash, listener state)
vane reload                        trigger reload of running daemon
vane shutdown                      graceful drain + exit
vane compile <DIR>                 dry-run compile; emit SymbolicFlowGraph JSON

# Snapshots (`get` group)
vane get config                    active SymbolicFlowGraph as JSON
vane get connections               in-flight connections snapshot
vane get metrics                   counter / gauge snapshot (default Prometheus text; `--json` for parsed)
vane get pools                     WASM + CGI pool occupancy
vane get upstreams                 cached TCP / TLS / QUIC upstream entries
vane get certs                     managed + static certs the daemon tracks

# Streams (`tail` group)
vane tail flow                     subscribe to FlowLogEvent broadcast (NDJSON)
vane tail log                      subscribe to structured tracing log (NDJSON)

# Certificates (`cert` group)
vane cert renew <SNI>              force-renew one managed cert (bypasses the renewal timer)

# Pools (`pool` group)
vane pool drain <FINGERPRINT>      drop one cached upstream entry by id

# TUI
vane tui                           launch TUI (requires `tui` feature)
```

CLI subcommand → wire verb mapping is one-to-one and mechanical: `vane get config` calls `get_config`, `vane cert renew` calls `force_renew`, `vane pool drain` calls `pool_drain`. The CLI does not hide or rename verbs; it nests for ergonomics.

## Output modes

Each `clap`-dispatched subcommand has two output modes:

- `--json` — emits the management verb's `result` verbatim (or a defined machine-readable shape for CLI-local commands). Assertable via `jq` piping.
- Default (pretty) — human-friendly tables / trees. Auto-disabled under `!isatty(stdout)` so piping to a script never produces ANSI escapes by accident.

## Connection surface

Both transports, configured via flags:

```
vane <command> --socket /var/run/vaned.sock                 Unix socket
vane <command> --http https://admin.example.com --token-env Y   HTTP-over-TCP
```

The TUI accepts the same flags — see [`../tui.md`](../tui.md).

## Tests

CLI subcommands are tested via `assert_cmd::Command::cargo_bin("vane")` plus `predicates`. JSON output is asserted via `jq` piping; pretty output is asserted on stdout fragments.

The TUI's pure layer (data adapters, view state machine, input mapping) is unit-tested in-crate. Rendering and crossterm side-effects are verified interactively — the LLM/human split.
