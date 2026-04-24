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

### ListenSpec grammar

| Form              | Expands to                                            | Semantics                                                        |
| ----------------- | ----------------------------------------------------- | ---------------------------------------------------------------- |
| `":443"`          | `0.0.0.0:443` + `[::]:443` (two entries, same NodeId) | Dual-stack; two independent listeners sharing one graph entry    |
| `"*:443"`         | same as `":443"`                                      | Alias                                                            |
| `"0.0.0.0:443"`   | `0.0.0.0:443`                                         | IPv4 only                                                        |
| `"[::]:443"`      | `[::]:443`                                            | IPv6 only; `bindv6only=1` (no IPv4-mapped); see `01-topology.md` |
| `"127.0.0.1:443"` | as written                                            | Specific IPv4 bind                                               |
| `"[::1]:443"`     | as written                                            | Specific IPv6 bind                                               |
| `":0"` / `"*:0"`  | **rejected at compile**                               | Wildcard port disallowed — graph entry keys must be stable       |

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
- `http.body.contains`, `http.body.matches` — HTTP body-level (triggers LazyBuffer).
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
VANE_DATA_DIR=/var/lib/vaned
VANE_CONFIG_DIR=/etc/vaned
VANE_LOG_LEVEL=info
VANE_MANAGEMENT_UNIX=/var/run/vaned.sock

# Address-family toggles. Default 1 = bind this family. Set to 0 to globally
# suppress — useful on hosts where one stack is disabled at the kernel level.
# Affects ":PORT" dual-stack expansion and explicit [::]:/0.0.0.0: binds.
VANE_BIND_IPV4=1
VANE_BIND_IPV6=1

# L1 security floors (configurable upward, floors enforced at compile)
VANE_SEC_MAX_HEADER_BYTES=65536
VANE_SEC_MAX_HEADERS_COUNT=100
VANE_SEC_HEADER_TIMEOUT=30
VANE_SEC_MAX_CONN_PER_IP=100

# Management transports (Unix always bound; HTTP only if BIND is set)
VANE_MGMT_UNIX=/var/run/vaned.sock
VANE_MGMT_HTTP_BIND=                       # empty = Unix-only; set to bind HTTP, e.g., 127.0.0.1:4479
VANE_MGMT_HTTP_TOKEN=                      # required when VANE_MGMT_HTTP_BIND is non-loopback
VANE_MGMT_HTTP_TLS_CERT=
VANE_MGMT_HTTP_TLS_KEY=

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
