# Configuration

## Format: JSON only

No YAML, no TOML, no DSL. Justification:

- JSON has exactly one canonical parser (`serde_json`). YAML and TOML have multiple with divergent behaviors.
- The internal config struct is the canonical schema. JSON is a serialization of it.
- The CLI and TUI generate configuration by serializing the internal struct, not by templating text.
- Users may hand-write JSON, but the recommended path is to generate it via `vane`.

## Directory layout

```
/etc/vaned/
├── config.json            # global vaned settings
├── rules/                 # rule files
│   ├── 00-listeners.json
│   ├── 10-web-api.json
│   └── 20-ssh-fallback.json
└── wasm/                  # WASM plugin binaries
    └── my-plugin.wasm
```

Every `.json` under `rules/` contributes to the merged rule set. The `NN-name.json` pattern is a convention enforced by lexical sort; it is not parsed specially.

## Top-level file schema

```json
{
  "order": 10,
  "rules": [ { ... }, { ... } ]
}
```

Or, when a file carries global settings:

```json
{
  "order": 0,
  "listeners": [ { ... } ],
  "management": { "unix_socket": "...", "http": null },
  "wasm": {
    "my-plugin": { "stateless": false, "pool": 4 }
  }
}
```

A file may contain any subset: just `rules`, just `listeners`, or a mix.

## Rule schema

```json
{
	"rule": "web-api",
	"listen": [":443"],
	"match": [
		{ "tls.sni": { "equals": "api.example.com" } },
		{ "http.header.host": { "equals": "api.example.com" } }
	],
	"terminate": {
		"type": "http_proxy",
		"upstream": "127.0.0.1:8080",
		"version": "auto",
		"timeouts": { "connect": "5s", "total": "60s" }
	}
}
```

- `rule` (string, required, unique across the entire merged set).
- `listen` (array of strings, required): port specs — see **ListenSpec grammar** below.
- `match` (array, optional): zero or more predicates. All must hold. Empty means "always match."
- `terminate` (object, required): see [`05-terminator.md`](05-terminator.md).

When `terminate.type` is one of the proxy variants (`http_proxy`, `http1_proxy`, `http2_proxy`, `http3_proxy`), the `version` field selects the upstream HTTP version:

| `version` | Wire behavior                                                                                                               |
| --------- | --------------------------------------------------------------------------------------------------------------------------- |
| `"auto"`  | TLS upstream: ALPN offers `["h2", "http/1.1"]`, hyper picks the negotiated version. Cleartext upstream: falls back to `h1`. |
| `"h1"`    | Force HTTP/1.1. ALPN offers `["http/1.1"]` only on TLS.                                                                     |
| `"h2"`    | Force HTTP/2. ALPN offers `["h2"]` only on TLS; cleartext uses prior-knowledge h2c (Stage 2).                               |
| `"h3"`    | Force HTTP/3 over QUIC. Stage 3 only — rules using `"h3"` on a binary built without the `h3` feature fail at compile.       |

Omitting `version` defaults to `"auto"`. The four type aliases (`http_proxy` / `http1_proxy` / `http2_proxy` / `http3_proxy`) are equivalent to `http_proxy` with `version` set to `auto` / `h1` / `h2` / `h3` respectively — see [`05-terminator.md`](05-terminator.md) § _Variant ergonomics in config_.

The proxy variants also accept an optional `dns` field. Upstream address resolution defaults to the system resolver (`/etc/resolv.conf` plus `/etc/hosts`) — see [`07-l7.md`](07-l7.md) § _DNS resolver: `hickory-resolver`_. A rule may override this with explicit nameservers:

| `dns`                                          | Wire behavior                                                                                                                                            |
| ---------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- |
| omitted / `null` / `"system"` / `{}`           | System resolver (reads `/etc/resolv.conf`).                                                                                                              |
| `{ "nameservers": ["1.1.1.1", "8.8.8.8:53"] }` | Resolve only against the listed servers, in order. Per-entry strings are `IP` (port `53` implied), `IPv4:port`, or `[IPv6]:port`. Bare IPv6 is rejected. |

Override semantics are **strict** — a query failure against the explicit nameservers does not fall back to the system resolver, it surfaces as `DnsFailure`. This is deliberate: split-horizon deployments where the operator pinned an internal DNS would consider a fall-through to public resolvers a leak, not a safety net. Override order is preserved end-to-end: `["10.0.0.1", "10.0.0.2"]` and `["10.0.0.2", "10.0.0.1"]` produce distinct upstream-client cache slots so the operator's primary / secondary intent is honored.

There is intentionally **no `listener_kind` (or `kind`) field** here. `ListenerKind` is derived from each listener's FlowGraph entry subgraph at compile — `Raw` when only L4 fetches are reachable, `Http` when only L7, `Auto` when both. A rule writer expresses intent by choosing terminators / presets; the listener's runtime posture follows. See [`06-l4.md`](06-l4.md) § _Listener kind derivation_ for the full rule.

### ListenSpec grammar

A listen entry has the form `[<transport>:]<address>`. The transport prefix declares the wire transport for that listener; the address form is independent.

#### Transport prefix

| Prefix   | Transport | Notes                                                                                          |
| -------- | --------- | ---------------------------------------------------------------------------------------------- |
| `tcp:`   | TCP       | Explicit. CLI / TUI emits this form for TCP listeners.                                         |
| `udp:`   | UDP       | Required for UDP listeners (H3 termination, L4 UDP forward, DNS-over-UDP forward).             |
| _(none)_ | TCP       | Implicit default. Equivalent to `tcp:`. Bare entries remain valid for backwards compatibility. |

The prefix is the **listener's** wire transport. It is independent of the upstream transport — an HTTP rule on a `tcp:` listener may proxy to a QUIC upstream (and vice versa) since the two sides never meet at the transport layer (see `07-l7.md` § _Architecture: TCP / QUIC separation_).

When the listener transport conflicts with the rule's reachable fetches the compiler rejects it:

- A `tcp:` listener with any reachable `L4Forward { transport: "udp" }` fetch → compile error.
- A `udp:` listener with any reachable `L4Forward { transport: "tcp" }` fetch → compile error.
- A `udp:` listener whose graph reaches only L7 fetches derives `ListenerKind::Http` (= H3-over-QUIC), per `06-l4.md` § _UDP listener semantics_. No fetch-side transport field is consulted; the listener's prefix is authoritative.

#### Address forms

| Form              | Expands to                                            | Semantics                                                        |
| ----------------- | ----------------------------------------------------- | ---------------------------------------------------------------- |
| `":443"`          | `0.0.0.0:443` + `[::]:443` (two entries, same NodeId) | Dual-stack; two independent listeners sharing one graph entry    |
| `"*:443"`         | same as `":443"`                                      | Alias                                                            |
| `"0.0.0.0:443"`   | `0.0.0.0:443`                                         | IPv4 only                                                        |
| `"[::]:443"`      | `[::]:443`                                            | IPv6 only; `bindv6only=1` (no IPv4-mapped); see `01-topology.md` |
| `"127.0.0.1:443"` | as written                                            | Specific IPv4 bind                                               |
| `"[::1]:443"`     | as written                                            | Specific IPv6 bind                                               |
| `":0"` / `"*:0"`  | **rejected at compile**                               | Wildcard port disallowed — graph entry keys must be stable       |

Address forms compose with transport prefixes by concatenation: `udp:443`, `tcp:0.0.0.0:443`, `udp:[::]:443`, `tcp:[::1]:443`. The parser strips the `tcp:` / `udp:` prefix when present and parses the remainder as one of the address forms above.

Dual-stack expansion produces two `entries` map keys (v4 and v6 `SocketAddr`s) pointing to the **same** `NodeId`. Bind happens independently per listener; `01-topology.md` defines the tolerance for one-side failure (warn + continue if only one family binds; fail the rule only if both fail).

The env vars `VANE_BIND_IPV4=0` / `VANE_BIND_IPV6=0` (default `1`) globally suppress one family — useful on hosts where the kernel has disabled one stack entirely and repeated bind attempts just produce noise. With `VANE_BIND_IPV6=0`, dual-stack `listen` specs expand to v4-only; explicit `[::]:PORT` specs fail at validate with a clear error. The flags are symmetric.

## Predicate schema

```json
{ "<field-path>": { "<operator>": <value> } }
```

Field paths (proposal):

- `transport`, `remote`, `local` — connection-level.
- `tls.sni`, `tls.alpn`, `tls.version` — TLS-level.
- `http.method`, `http.uri.path`, `http.uri.query`, `http.header.<name>` — HTTP header-level.
- `http.body` — HTTP body-level (triggers LazyBuffer). Operators `contains` / `matches` / etc. apply to this field in the usual operator-object form. See `18-predicate-schema.md` for the authoritative grammar.
- `peek` — L4 peek buffer (byte match).

Operators:

- `equals`, `not_equals`
- `contains`, `not_contains` — strings, byte arrays
- `matches` — regex. Engine: `fancy-regex` (supports lookaround and backreferences). Patterns have a compile-time size limit (rejected if the compiled NFA exceeds a threshold) and a runtime `backtrack_limit` that hard-caps backtracking steps. Patterns using no fancy features delegate to the `regex` crate internally and run in guaranteed linear time.
- `prefix`, `suffix`
- `in`, `not_in` — list membership
- `gt`, `lt`, `gte`, `lte` — numeric
- `cidr` — IP address membership in a CIDR range

Operator LHS and RHS have declared types; a type mismatch is a compile error.

Combinators:

- Top-level `"match": [A, B, C]` is implicit AND. All predicates must hold.
- `{ "any_of": [A, B, C] }` — OR; any one must hold.
- `{ "not": A }` — negation.

These three compose to express any boolean predicate (De Morgan). `all_of` is deliberately absent; the top-level array already expresses it.

## Merge

1. Scan `rules/` (lex sort on path).
2. Read `order` (default 0) from each file's top level.
3. Stable-sort files by `(order asc, filename lex)`.
4. Accumulate:
   - Rules: concatenate. Duplicate `rule` names are a **merge error** (explicit, not silent override).
   - Global settings: last-write-wins with a log entry `"field X in file Y overridden by file Z"`.
5. Emit a single `MergedConfig`.

The merge is deterministic and reproducible across machines.

## Compile

`MergedConfig → FlowGraph` — full procedure in [`02-flow.md`](02-flow.md).

## Reload

1. File watcher (`notify` 6.x + `notify-debouncer-full` 0.3+) observes `/etc/vaned/` recursively. Raw `notify` emits multiple events per editor save on many filesystems (write → rename → chmod); the debouncer collapses a burst into one event. Debounce timeout defaults to **250 ms** (≥ the filesystem's atomic-write tolerance; raise for slow NFS). A batch touching multiple files in one burst merges into a single reload because merge reads the whole directory every time.
2. Any change triggers re-merge + re-compile.
3. On success: `ArcSwap` replaces the active FlowGraph. In-flight connections keep the old graph. Old graph drops when its last `Arc` releases.
4. On failure: error logged to stderr/journal and surfaced via management API. Active FlowGraph unchanged.

`vane reload` triggers the same pipeline without waiting for the file watcher.

### Watcher arm-up ordering

The file watcher **must not** observe filesystem events until `ListenerSet::start` has returned and at least the initial `Arc<FlowGraph>` is installed in the daemon's `ArcSwap`. Boot sequence is therefore strict:

```
parse args → init tracing → load + compile + link  (initial graph)
           → ListenerSet::start                    (accept loops spawned)
           → spawn_watcher                         (debouncer registered)
           → wait_for_shutdown_signal
```

`spawn_watcher` is the last setup step. Any reload event raced ahead of listener bind would have nothing useful to do — the active graph is the boot graph either way — but the strict ordering rules out malformed states where the watcher fires before the daemon is internally consistent. If the watcher's underlying notify registration fails (typically permission-denied at the directory level), the daemon logs a warning and continues without auto-reload; reload is then driven by `vane reload` against the management socket, or by daemon restart.

### Watched events: filtered to file-level mutations only

The watcher cares about exactly three filesystem signals on `<config_dir>/`:

| Signal                | Reload action                    |
| --------------------- | -------------------------------- |
| File created          | re-run merge → may add a rule    |
| File content modified | re-run merge → rule body change  |
| File deleted          | re-run merge → may remove a rule |

All other notify events — directory metadata changes, attribute changes (chmod, chown), access timestamps, mount events, sub-directory changes outside `rules/` — are **ignored**. The debouncer collapses bursts inside the 250ms window; the post-debounce filter keeps only file-level mutations under the watched tree before invoking `reload_once`. Spurious events that pass notify (e.g., editor swap-file dance) are tolerated because the post-reload `version_hash` idempotency check skips the `ArcSwap::store` when content is semantically unchanged — but at the watcher layer we still want minimal noise reaching `reload_once` to keep the CPU cost of "no-op" reloads low.

### NodeId stability across reloads

`SymbolicFlowGraph::entries` maps `SocketAddr → NodeId`. Two successive compiles on related-but-different rule sets allocate `NodeId`s independently — the lower pass numbers nodes in iteration order, and a single edited rule shifts every later allocation. **Listener accept loops therefore must not capture a boot-time `NodeId` and reuse it across reloads.** The contract is:

- The accept loop holds an `Arc<ArcSwap<FlowGraph>>` (cheap to clone, lock-free to `load`).
- On each accept: load the current graph (`load_full()`), then look up the entry by the listener's `local_addr` in `graph.symbolic().entries`. The result is a fresh `NodeId` valid against the graph the connection will execute on.
- If the address is no longer present in the active graph (the rule was deleted or moved to a different listener spec by a reload), the connection is closed immediately — the client sees a TCP RST, just as it would for a no-rule listener at boot.

This rule covers two failure modes that captured-NodeId-at-boot would silently mishandle: an edited rule whose graph re-allocates `NodeId`s (incoming traffic would route through whichever node now happens to occupy the old id — typically wrong), and a deleted rule whose listener is still bound on the daemon side (incoming traffic would walk a graph that has no entry for it — undefined).

Listener-set diff is performed by `ListenerSet::reconcile`, called by the watcher's reload pipeline immediately after a successful `ArcSwap::store`. Addresses present in the new graph but not currently bound spawn fresh accept loops (same wiring as boot, including bind-retry); addresses present currently but absent from the new graph fire `accept_cancel`, drain in-flight in the background up to a 30s budget (mirroring the SIGTERM drain default), then escalate to `force_cancel` and abort. The reconcile call itself returns immediately so file-watcher reloads never stall on long-lived tunnels. Addresses unchanged across the reload keep running; their per-accept `entries.get(&addr)` lookup picks up the new graph's `NodeId` on the next accept.

### Compiled artifact: in-memory only

`vaned` re-runs the full compile pipeline (`merge → expand → analyze → lower → validate → link`) on every boot and on every reload. The compiled `FlowGraph` exists only in process memory and is never persisted to disk. Rationale:

- **Single source of truth.** JSON files are the authoritative configuration; the in-memory graph is derived. A persisted compiled artifact would be a third state that can desynchronise from either.
- **Schema fragility.** Every IR change (new node kind, new field, hash-cons key change) would require explicit cache versioning + invalidation. Forgetting once produces silent miscompiles in production.
- **Performance is a non-issue.** The pipeline is sub-millisecond for typical rule counts and dominated by JSON parsing, which is already the same work `dry-run` does. Saving boot time at the cost of a persistence layer is a wrong trade for a network-proxy daemon.

Operators who want to inspect the compiled state query the management API (`get_config` returns the active `SymbolicFlowGraph` as JSON). `vane compile --dry-run /path/to/dir` runs the same pipeline without binding listeners, producing the same JSON, for review of a proposed deploy before swap.

## `vane compile --dry-run`

Reads `/etc/vaned/`, merges, compiles through the core pipeline (`merge → expand → analyze → lower → validate`), and emits the resulting `SymbolicFlowGraph` as JSON to stdout. No interaction with a running `vaned`. Used to:

- Review a proposed merge before committing.
- Debug unexpected rule interactions.
- Compare current vs. proposed state.

The output is the **symbolic** form — pure IR, no `Arc<dyn _>` trait objects (which are not serializable anyway). This means dry-run runs against `vane-core` only; it does not link hyper / rustls / wasmtime and does not require an engine build. The same pipeline runs inside `vaned` on boot and reload, after which engine's `link` step constructs the runtime `FlowGraph` from the symbolic one (see `02-flow.md` § _Compile and link_).

The compiled JSON is deterministic given the same input.

## Three-layer configuration

`vane` separates configuration into three layers by change frequency:

| Layer                | Location                          | Change cadence    | Effect mechanism        |
| -------------------- | --------------------------------- | ----------------- | ----------------------- |
| Deployment constants | `/etc/vaned/.env` (via `dotenvy`) | Deploy-time, rare | Daemon restart required |
| Daemon-scoped config | `/etc/vaned/config.json`          | Occasional        | Reload                  |
| Flow rules           | `/etc/vaned/rules/*.json`         | Frequent          | File-watch auto-reload  |

### Deployment constants (`.env`)

Loaded once at daemon startup via `dotenvy`. Values frozen for the process lifetime; changes require restart.

Content: things that describe the runtime environment rather than the traffic model. Paths, credentials, L1 security limits (see `13-rate-limit.md`), log levels, management bindings.

```
# /etc/vaned/.env
VANE_CONFIG_DIR=/etc/vaned                 # honored when `vaned --config` is omitted
VANE_LOG_LEVEL=info                        # tracing-subscriber filter (RUST_LOG overrides)
VANE_WASM_DIR=                             # default `<config-dir>/wasm`

# Address-family toggles. Default 1 = bind this family. Set to 0 to globally
# suppress — useful on hosts where one stack is disabled at the kernel level.
# Affects ":PORT" dual-stack expansion and explicit [::]:/0.0.0.0: binds.
VANE_BIND_IPV4=1
VANE_BIND_IPV6=1

# Listener bind-retry and drain tuning
VANE_BIND_MAX_ATTEMPTS=10         # per-address bind-retry count (01-topology.md § Bind)
VANE_BIND_BACKOFF_INITIAL_MS=100  # initial retry backoff in milliseconds
VANE_BIND_BACKOFF_MAX_MS=5000     # retry backoff cap in milliseconds
VANE_FORCE_CANCEL_GRACE_SECS=5    # secondary grace window after force_cancel before abort
VANE_DRAIN_TIMEOUT_SECS=30        # in-flight drain budget — SIGTERM and removed-listener reconcile

# Boot health watchdog. After listeners.start, the daemon polls each
# listener's bind-ready flag for up to this many seconds. If zero
# listeners have bound by the deadline, vaned exits non-zero (no point
# running with no service). Partial bind (some succeeded, some failed)
# logs WARN and the daemon continues. Default 60s — covers the
# bind-retry budget (VANE_BIND_MAX_ATTEMPTS × VANE_BIND_BACKOFF_MAX_MS).
VANE_BOOT_HEALTH_TIMEOUT_SECS=60

# L1 security floors (configurable upward, floors enforced at compile)
VANE_SEC_MAX_HEADER_BYTES=65536
VANE_SEC_MAX_HEADERS_COUNT=100
VANE_SEC_HEADER_TIMEOUT=30
VANE_SEC_MAX_CONN_PER_IP=100
VANE_SEC_MAX_TOTAL_CONNS=65536

# Management transports — Unix always bound; HTTP plaintext, default-on at port 3333.
# `VANE_BIND_IPV4` / `VANE_BIND_IPV6` decide which families participate
# (see ListenSpec section). Empty `HTTP_PORT` disables the HTTP transport
# (Unix-only mode). `HTTP_PUBLIC` flips bind from loopback (default) to
# wildcard. `HTTP_TOKEN` is mandatory when `HTTP_PUBLIC` is set; loopback
# bind without a token is allowed but warns at boot. See 10-management.md
# § _Auth model_ for the full table and the recommended TLS deployment
# (vane reverse-proxies its own admin endpoint).
VANE_MGMT_UNIX=/tmp/vaned.sock
VANE_MGMT_HTTP_PORT=3333                   # empty = HTTP transport disabled
VANE_MGMT_HTTP_PUBLIC=                     # empty / "0" / "false" = loopback only; truthy = wildcard
VANE_MGMT_HTTP_TOKEN=                      # bearer token; mandatory when HTTP_PUBLIC is truthy

# CGI (if `cgi` feature enabled)
VANE_CGI_MAX_CONCURRENT=100
# ...
```

Environment variables use the `VANE_` prefix. Namespace prefixes under that (`SEC_`, `MGMT_`, `WASM_`, etc.) group related settings.

### Daemon-scoped config (`config.json`)

JSON. Describes listener bindings, management transport config, WASM plugin pool sizes, cert populator configuration — things that shape the daemon's service surface but are not per-rule.

Reload applies these — no restart needed for most changes.

### Flow rules (`rules/*.json`)

Per-rule JSON files. Frequently edited. File watcher detects changes and triggers recompile + ArcSwap.

## Configuration vs. runtime state

Configuration is file-backed, user-authored, version-controllable. It is the source of truth for declared intent.

Runtime state (connection counts, WASM pool occupancy, per-upstream RTT, reload history) is daemon-held and never written back to configuration files. State is queryable via the management API.

## HMR granularity

The swap unit is the whole `FlowGraph`. Reload replaces the entire graph atomically.

Per-listener or per-rule partial swaps are not supported. Reasons:

- Correctness of a FlowGraph is holistic — compile-time optimizations (shared predicate prefixes, LazyBuffer decisions) cross rule boundaries.
- Atomic whole-graph swap is O(1) and obviously correct. Partial swap with consistency guarantees is hard.
- Re-merge + re-compile is sub-millisecond for realistic rule counts.

If reload latency becomes a bottleneck (thousands of rules, sub-millisecond deadlines), this is revisitable. MVP: full swap.
