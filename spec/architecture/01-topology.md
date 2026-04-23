# Topology

## Processes

Vane ships two binaries:

- **`vane`** — user-facing terminal binary. Two modes:
  - **CLI**: stateless short-lived invocation. Generates and modifies configuration files, queries a running daemon, dry-runs compilation, triggers reload.
  - **TUI**: long-lived interactive session. Connects to one `vaned` instance, observes live connections, inspects flow paths, tails logs.
- **`vaned`** — long-lived daemon. Owns listeners, executes FlowGraphs, manages WASM instances, maintains connection state.

CLI and TUI share the same client library. TUI is a higher-level shell over CLI capabilities plus streaming subscriptions.

## Management transports

`vaned` exposes exactly one management protocol on two transports:

- **Unix domain socket** (default-on). Path: `$XDG_RUNTIME_DIR/vaned.sock`, falling back to `/var/run/vaned.sock`. No authentication — filesystem permissions are the boundary.
- **HTTP-over-TCP** (opt-in, off by default). Bound to `127.0.0.1:PORT` by default; bind address is configurable. Bearer-token authentication. TLS required when bound to non-loopback.

The wire protocol is identical on both transports. See [`10-management.md`](10-management.md). The TUI is a client of this protocol, not a privileged in-process consumer.

## Daemon lifecycle

1. **Boot** — parse bootstrap config directory, merge, compile to FlowGraph, bind listeners (with retry), start management socket. If compile fails at boot, `vaned` exits with a non-zero status and writes the error to stderr; no partial start. Individual listener bind failures (after retry exhaustion) are logged and tolerated — other listeners are unaffected.
2. **Run** — accept connections, dispatch to FlowGraph, swap configurations atomically on reload.
3. **Reload** — file watcher or explicit management call triggers re-merge + re-compile. On success, `ArcSwap` replaces the active FlowGraph, and the listener set is diffed against the previous one (see "Listener lifecycle" below). On failure, the old graph and listener set continue serving; the error is surfaced via the management API and logs.
4. **Shutdown** — stop accepting new connections, wait for in-flight connections to drain or timeout (`drain_timeout`, default 30s), close management socket, exit.

## Listener lifecycle

Listeners are independent tokio tasks per `(transport, address)` pair. Each task owns its accept socket and holds a `CancellationToken` tied to its liveness. **Listener configuration changes occur only at two points: boot and reload.** No other code path adds or removes a listener.

### Bind

At boot and when reload introduces a new listener:

- `SO_REUSEADDR` is on by default (fast restart through TIME_WAIT).
- `SO_REUSEPORT` is off by default (opt-in for multi-process deployments).
- On bind failure, the task retries with **exponential backoff** (100 ms → 5 s cap) up to a configured `max_bind_attempts` (default 10). If retries exhaust, the task gives up on that specific address and logs — other listeners are unaffected.

### Accept loop

Each accepted connection spawns a per-connection task holding an `Arc<FlowGraph>` captured at accept time. Accept-level errors (`EMFILE` and similar) use the same exponential backoff, then resume.

### Reload diffing

On successful FlowGraph recompile, the daemon diffs the next compiled listener set against the currently bound set:

- **Listeners unchanged** — the task keeps running. FlowGraph is already ArcSwapped; the next accept on this listener sees the new graph.
- **Listeners added** — new tokio tasks spawn with the bind-retry logic above.
- **Listeners removed** — the task receives cancellation:
  1. Accept socket closes immediately (no new connections).
  2. Existing in-flight connections on this listener are **not forcibly closed**. They run to natural completion against their captured `Arc<FlowGraph>`.
  3. The task waits up to `drain_timeout` (default 30 s) for drain.
  4. On timeout, remaining connections are aborted.
  5. Task releases the socket and exits.

No in-flight connection ever observes a torn-down graph. Listener removal is soft-first, forced only on drain timeout.

### IPv4 / IPv6

Dual-stack is expressed by **two separate listeners**, one per address family. A config wanting to serve both IPv4 and IPv6 on port 443 lists both `0.0.0.0:443` and `[::]:443`.

Single-bind on `[::]` with OS-level IPv4-mapping is deliberately not supported:

- Debian/Ubuntu default `net.ipv6.bindv6only=1` disables this behavior entirely.
- Peer addresses arrive as `::ffff:a.b.c.d`, breaking rules that match on `remote.ip == "a.b.c.d"` without an unmap layer.
- Cross-platform behavior is inconsistent (Linux, macOS, Windows differ).

Explicit separation matches nginx/Caddy/HAProxy production defaults.

### Shutdown

SIGTERM triggers cancellation on all listener tasks simultaneously. Same drain semantics: stop accepting, let in-flight finish up to `drain_timeout`, abort the rest, exit.

SIGKILL bypasses all of this (OS-level kill; no graceful behavior possible).

## Privileges and socket binding

For ports below 1024, `vaned` needs elevated privileges. Two supported mechanisms:

- **`CAP_NET_BIND_SERVICE`** — the systemd unit grants this capability; `vaned` runs as an unprivileged user and can still bind low ports. Recommended default.
- **Systemd socket activation** — systemd binds the listening sockets and passes file descriptors via `sd_listen_fds`. `vaned` never binds anything itself.

Bind-then-drop (starting as root, binding, dropping to an unprivileged user) is **not supported**. The two approaches above cover all production cases without the startup-as-root security surface.

## Filesystem layout

Defaults below; all overridable via `config.json` or environment variables.

```
/etc/vaned/              # bootstrap config (user-authored, source of truth)
  config.json            # global vaned config
  rules/*.json           # one or many rules per file, merged at boot/reload
  wasm/*.wasm            # WASM plugin binaries referenced by rules

/var/lib/vaned/          # daemon-owned state
  compiled.json          # last-successful compiled FlowGraph (for post-crash recovery inspection)
  wasm/*.cwasm           # pre-compiled wasmtime modules

/var/run/vaned.sock      # Unix management socket, or $XDG_RUNTIME_DIR/vaned.sock
/var/log/vaned.log       # structured log (plain text when not using journald)
```

## Bootstrap vs. live state

`vane` the CLI operates in two modes:

- **Offline** — reads and writes files in `/etc/vaned/` directly. Used for provisioning before `vaned` is running, or for git-managed configuration.
- **Online** — connects to a running `vaned`. Queries live state, pulls the compiled FlowGraph for inspection, triggers reload.

Configuration is always file-authoritative. Online CLI mutations (if any) still go to files on disk; `vaned` observes via the file watcher. The daemon is not a configuration database.

Live state (connection counts, WASM pool occupancy, per-upstream RTT statistics, hot-reload history) is daemon-held and exposed only via the management API.

## Relationship between configuration files and runtime

```
/etc/vaned/rules/*.json  ──(merge)──►  MergedConfig  ──(compile)──►  Arc<FlowGraph>
                                                                           │
                                                                     ArcSwap::store
                                                                           │
                                                                           ▼
                                                                   active FlowGraph
                                                                           │
                                                            (each new connection reads here)
```

In-flight connections hold an `Arc<FlowGraph>` captured at accept time. A reload that produces a new FlowGraph does not affect them. The old FlowGraph drops when its last `Arc` is released.
