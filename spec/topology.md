# Topology

## Processes

Vane ships two binaries:

- **`vane`** — user-facing terminal. Stateless CLI for short-lived calls, plus a long-lived TUI mode that connects to one running `vaned`. Source: [`crates/cli/`](../crates/cli/). See [`crates/cli.md`](crates/cli.md), [`tui.md`](tui.md).
- **`vaned`** — long-lived daemon. Owns listeners, executes FlowGraphs, manages WASM instances, holds connection state. Source: [`crates/daemon/`](../crates/daemon/). See [`crates/daemon.md`](crates/daemon.md).

CLI and TUI link the same management client (`vane-mgmt`); both speak the protocol described in [`crates/mgmt.md`](crates/mgmt.md).

## Filesystem layout

```
/etc/vaned/                 # bootstrap config (operator-authored, source of truth)
  config.json               # global daemon settings
  rules/*.json              # one or many rules per file, merged at boot/reload
  wasm/*.wasm               # plugin binaries referenced by rules
  .env                      # deploy-time constants (dotenvy)

/var/lib/vaned/             # daemon-owned state
  wasm/*.cwasm              # pre-compiled wasmtime modules (content-hash keyed)
  acme/                     # ACME accounts and issued certs (see crates/engine-acme.md)

/var/run/vaned.sock         # Unix mgmt socket (or $XDG_RUNTIME_DIR/vaned.sock)
/var/log/vaned.log          # structured log (when not using journald)
```

Compiled FlowGraph artifacts are not persisted. The pipeline runs on every boot and reload. Operators inspect compiled state via `vane compile <DIR>` (dry-run via `compile_dry_run` mgmt verb against a running daemon) or `get_config` (live).

## Daemon lifecycle

1. **Boot** — parse args, load `.env` (OS env wins), install crypto provider, init tracing, scan config, compile + link, bind listeners, start mgmt server, spawn watcher. Compile failure exits non-zero. Per-listener bind failures are logged; other listeners continue. See `crates/daemon/src/main.rs`.
2. **Run** — accept, dispatch, swap atomically on reload.
3. **Reload** — file watcher or `vane reload` triggers re-merge + re-compile. On success, `ArcSwap` replaces the active FlowGraph; the listener set diffs against the previous one. On failure, the old graph and listener set continue serving; the error surfaces via mgmt.
4. **Shutdown** — SIGTERM stops accepting, drains in-flight up to `VANE_DRAIN_TIMEOUT_SECS` (default 30s), closes mgmt, exits. SIGKILL bypasses drain.

## Listener lifecycle

Listeners are independent tokio tasks per `(transport, address)`. Each owns its accept socket and a `CancellationToken`. Listener-set changes happen at boot and reload — no other code path adds or removes a listener. Source: `crates/engine/src/listener.rs`, `crates/engine/src/listener_udp.rs`.

### Bind

- `SO_REUSEADDR` on by default.
- `SO_REUSEPORT` off by default.
- Bind failure retries with exponential backoff (`VANE_BIND_BACKOFF_INITIAL_MS` → `_MAX_MS`, default 100 ms → 5 s) up to `VANE_BIND_MAX_ATTEMPTS` (default 10). On exhaustion, that listener gives up; siblings are unaffected.

### Reload diff

- **Unchanged** — task continues; next accept sees the new graph via `ArcSwap`.
- **Added** — fresh task spawns with bind retry.
- **Removed** — accept socket closes immediately; in-flight connections run to completion against their captured `Arc<FlowGraph>`. Drain budget `VANE_DRAIN_TIMEOUT_SECS`; remaining connections abort on timeout.

No in-flight connection ever observes a torn-down graph.

### What reload does not preserve

The HMR contract is "in-flight connections see no disruption". Graph-scoped stateful state (rate-limit buckets, counters, stateful WASM linear memory) does **not** migrate — the old graph's `MiddlewareInst`s drop with the old `Arc`; the new graph constructs its stateful middleware fresh. See [`flow-model.md` § _State migration_](flow-model.md#state-migration-on-reload).

Daemon-scoped state — L1 floor counters, TLS ticket keys, upstream pools, ACME registry — is preserved across reloads. It lives on the daemon, not on the graph.

### IPv4 / IPv6

Dual-stack expressed as two listeners. `listen: ":443"` expands to `0.0.0.0:443` and `[::]:443`, both pointing to one `NodeId`. Single-bind on `[::]` with OS-level v4-mapping is not supported (`bindv6only=1` on Debian/Ubuntu, peer addresses arrive as `::ffff:a.b.c.d` breaking `remote.ip` predicates, cross-platform behavior diverges). Operators who want one family write `"0.0.0.0:443"` or `"[::]:443"`.

`VANE_BIND_IPV4=0` / `VANE_BIND_IPV6=0` globally suppress one family — useful when the kernel disables one stack. Suppressed-family binds are skipped at `lower` time with no warning; explicit binds for a suppressed family fail at validate.

Partial-bind tolerance: with both families requested and only one binding successfully, the daemon emits a single `WARN` and continues on the bound family. Both failing falls into the general bind-failure policy (logged, tolerated).

### Privileges

- `CAP_NET_BIND_SERVICE` granted by the systemd unit. Recommended default.
- Systemd socket activation. `vaned` consumes file descriptors from `sd_listen_fds`.

Bind-then-drop (start as root, drop privileges) is not supported.

### Shutdown

SIGTERM cancels every listener task. Same drain semantics: stop accept, let in-flight finish up to drain budget, abort the rest.
