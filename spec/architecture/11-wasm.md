# WASM Runtime

## Runtime: wasmtime

- Mature, actively developed, async-native via `Config::async_support`.
- `PoolingAllocator` designed for the exact instance-reuse pattern `vane` needs.
- Component Model support is stable (since 2024); `vane` commits to Component Model as the plugin ABI.

## ABI: Component Model (WIT)

Plugins are compiled as **components**, not traditional modules. The interface is defined in WIT; plugin authors use `wit-bindgen` to generate language-specific bindings; the host uses wasmtime's `bindgen!` macro to generate host-side bindings.

### Why Component Model, not bincode

- **Polyglot by construction** — plugins written in Rust, Go (TinyGo), AssemblyScript, JavaScript (via QuickJS), or other WASM-targeting languages all compile to the same component format.
- **Interface versioning** — adding fields to WIT records does not break old plugins.
- **Ecosystem alignment** — WASI, Cloudflare Workers, and the broader component-model tooling are transferable skills.
- Slightly higher per-call cost than raw bincode (typed marshaling) — negligible next to actual plugin work.

The commitment to polyglot is load-bearing: `vane`'s extension promise is "write in any language that compiles to WASM", and that only holds if the ABI is polyglot.

## WIT interface (architectural shape)

Exact field layouts will be fixed in a dedicated `spec/wasm-abi.md` once implementation begins. Architectural shape:

```wit
package vane:plugin;

world middleware {
    import vane:host/host;
    export plugin;
}

interface plugin {
    use types.{ metadata, plugin-input, decision, plugin-error };

    get-metadata: func() -> metadata;
    handle:       func(input: plugin-input) -> result<decision, plugin-error>;
}

interface types {
    record metadata {
        name:       string,
        version:    string,
        kind:       middleware-kind,
        stateless:  bool,
        needs-body: bool,
        inspects:   list<string>,         // field paths the plugin reads
    }

    enum middleware-kind {
        l4-peek, l4-bytes, l7-request, l7-response,
    }

    // plugin-input's schema varies by middleware-kind; decision variant mirrors
    // the Rust-side Decision enum.
}

interface host {
    log:             func(level: log-level, message: string);
    now-unix-ms:     func() -> u64;
    random:          func(buf-len: u32) -> list<u8>;
    metric-counter:  func(name: string, delta: u64);
    metric-gauge:    func(name: string, value: s64);

    http-fetch:      func(req: http-request) -> result<http-response, net-error>;
}
```

## Plugin metadata drives compilation

`get-metadata()` is called once at module load. The cached metadata feeds:

- **FlowGraph compilation** — `kind` determines phase placement; `needs_body` drives LazyBuffer; `inspects` feeds inspection-level analysis and rule sorting.
- **Pool allocation** — `stateless` chooses between `PoolingAllocator` (cheap reuse) and fixed pre-allocated pool.
- **Rule validation** — a rule using a plugin as `L7RequestMiddleware` must reference a plugin whose metadata `kind == l7-request`; mismatch is a compile error.

## Module lifecycle

```
/etc/vaned/wasm/*.wasm          # source components
/var/lib/vaned/wasm/*.cwasm     # wasmtime-compiled cache, keyed by content hash
```

### Boot

For each `.wasm` in the config directory:

1. Compute content hash.
2. If matching `.cwasm` exists, `Component::deserialize` it (microsecond-scale).
3. Otherwise `Component::from_file` compiles (millisecond-per-KB); persist to `.cwasm`.
4. Instantiate, call `get-metadata()`, cache metadata.

### Hot reload

File watcher detects a changed `.wasm`. Compile new `Component`; call `get-metadata()` on it.

- **Metadata unchanged** (`kind`, `stateless`, `needs_body`, and the exported middleware set all equal) — module swap only; `FlowGraph` is not recompiled. Module registry swaps the active `Component` for that `module_id`. For **stateless** plugins: new invocations rent instances from `PoolingAllocator` which transparently creates new instances against the new component; in-flight stateless invocations (if any) finish and their instances drop. For **stateful** plugins: the old instance pool continues to serve in-flight checkouts until they return naturally; new checkouts will construct against the new component. Old instances drop as they complete. (Note: stateful linear memory bound to the old component's layout does **not** migrate to the new component even if metadata is compatible — each instance's state lives only as long as that instance.)
- **Metadata changed** — triggers **full FlowGraph recompile** (because compilation decisions depend on metadata). The new graph ArcSwaps into place; old graph drops when its in-flight requests finish. All stateful linear memory on that module resets — see `04-middleware.md` § _State migration on reload_.

## Instance pool

Two modes, declared per plugin's `stateless` metadata:

### Stateless

- Backed by wasmtime `PoolingAllocator` with small per-instance memory budget (default 1 MiB).
- Each invocation rents an instance, runs, returns; next call gets fresh memory.
- Pool grows on demand; daemon-wide cap (default 32) applies across all stateless plugins.
- On exhaustion: drop connection with overload error.

### Stateful

- Declared with `pool: N` (N ≥ 1, default 4).
- N instances pre-allocated at module load. Each call checks out, invokes, returns — linear-memory state persists **within a single graph generation**.
- Pool size fixed; auto-scaling deferred.
- On exhaustion: drop connection with 503. Queueing deliberately not implemented.
- **On FlowGraph reload** (metadata changed, or any other recompile-triggering change): the pool drops with the old graph. The new graph pre-allocates a fresh pool of N empty-state instances. Linear memory **does not migrate**. This matches vane's general "no state migration across reload" posture — see `04-middleware.md` § _State migration on reload_ for the rationale and the recommended external-layer alternatives.

## Host function surface

The minimal whitelist:

| Function                             | Purpose                                    |
| ------------------------------------ | ------------------------------------------ |
| `log(level, message)`                | Structured log integration                 |
| `now-unix-ms() -> u64`               | Wall clock                                 |
| `random(buf-len) -> list<u8>`        | Cryptographic RNG                          |
| `metric-counter(name, delta)`        | Emit counter event                         |
| `metric-gauge(name, value)`          | Emit gauge event                           |
| `http-fetch(request) -> result<...>` | Outbound HTTP request via daemon's TcpPool |

**Not provided**: network (other than `http-fetch`), filesystem, environment variables, process/thread spawn. Plugins are pure logic; external observation goes through whitelisted host functions under daemon control.

## `http-fetch` details

Outbound HTTP requests on behalf of a plugin:

```wit
http-fetch: func(req: http-request) -> result<http-response, net-error>;

record http-request {
    method:     string,                     // GET / POST / ...
    url:        string,                     // full URL with scheme/host/path
    headers:    list<tuple<string, string>>,
    body:       list<u8>,                   // full body, non-streaming
    timeout-ms: option<u32>,                // defaults to the plugin's configured default
}

record http-response {
    status:  u16,
    headers: list<tuple<string, string>>,
    body:    list<u8>,                      // full body, truncated to max-body-size
}

variant net-error {
    dns-failure(string),
    connection-refused,
    timeout,
    tls-error(string),
    pool-exhausted,
    body-too-large,
    not-allowed(string),
    internal(string),
}
```

Key design points:

- **Full body, non-streaming**. Streaming inside a plugin is too complex for the `http-fetch` use case (request-response for decisions). `max-body-size` default: 1 MiB request, 1 MiB response, per-plugin configurable.
- **Failures are typed, not traps**. Plugin handles via `Result` matching — decide to fallback, retry, cache, or fail. Traps are reserved for panic-level plugin bugs.
- **Shares the daemon's TcpPool** — same fingerprint, same pool, same observability as Fetch upstreams. Cross-crate wiring goes through a trait defined in `vane-core` so `vane-wasm` does not depend on `vane-engine`:

  ```rust
  // vane-core
  #[trait_variant::make(HttpFetchBackend: Send)]
  pub trait HttpFetchBackendLocal {
      async fn fetch(&self, req: Request, limits: HttpFetchLimits) -> Result<Response, Error>;
  }
  ```

  `vane-engine` provides the concrete impl wrapping `TcpPool`; `vaned`'s startup injects an `Arc<dyn HttpFetchBackend>` into `WasmtimeRuntime` before loading any plugins. Tests substitute a mock backend for isolated `http-fetch` verification.
- **Default ClientConfig** uses System trust CAs. Per-plugin override: custom CA bundle, mTLS client cert, `VerifyMode`.
- **`allowed_hosts`** per plugin config; default `["*"]` (no restriction). Narrow as needed (e.g., `["auth.example.com", "*.internal"]`); requests outside the list return `not-allowed`.
- **Rate limiting** per plugin is architected (daemon-level token bucket keyed by plugin name) but deferred to post-MVP.

## Memory and time limits

| Resource      | Default | Cap                 | Configurable per plugin |
| ------------- | ------- | ------------------- | ----------------------- |
| Linear memory | 1 MiB   | 128 MiB daemon-wide | Yes                     |
| Per-call time | 10 ms   | —                   | Yes                     |

Time enforcement: **epoch-based preemption** (not fuel). Epoch has negligible steady-state overhead (checked at periodic ticks); fuel adds per-instruction cost. Wasmtime sets an epoch deadline per call; plugin work interrupts on exceeding.

## Trap and error handling

All abnormal plugin exits, handled uniformly:

| Condition                                   | Handling                                                                                                                         |
| ------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------- |
| Trap (OOB access, divide-by-zero, overflow) | Log plugin name + module hash + wasm stack (if available); failure                                                               |
| Memory budget exceeded                      | Same                                                                                                                             |
| Time budget exceeded (epoch preempt)        | Same                                                                                                                             |
| Malformed output (WIT decode fail)          | Same                                                                                                                             |
| Component returns `Err(plugin-error)`       | **Not a trap** — plugin-originated structured error; flows through the regular `Middleware::Error` path (see `04-middleware.md`) |

On any of the above conditions, the plugin invocation returns `Err(Error)` up through the `L7RequestMiddleware::run` (etc.) trait method. From there the standard **middleware error channel** takes over — the same mechanism that handles non-WASM middleware errors:

- `Err(_)` is distinct from `Ok(Decision::Short(_))`; the latter is an application-level refusal the plugin designed to produce.
- Routing uses `Node::Middleware.on_error`. Default (unset) is the fail-safe tombstone: L7 → `500 Internal Server Error`; L4 → close connection.
- Config-level `on_error: "close" | { "response": ... }` is available the same way as for internal middleware.

See `04-middleware.md` § _Two error channels, not one_ for the full semantics. The old "`on_plugin_fail` is a WASM-only fallback" formulation is subsumed by this unified mechanism.

### Dedup policy

WASM plugin kinds follow the middleware dedup policy from `02-flow.md`:

- **Stateless WASM** (`stateless: true`): **hash-consed**. Key = `(module_id, export_name, canonical_args_json)`. Two rules invoking the same export with the same args share one `MiddlewareId`. The runtime's `PoolingAllocator` reuses Instances across the shared `MiddlewareId` invocations.
- **Stateful WASM** (`stateless: false`): **never deduped**. Each call site gets its own `MiddlewareId`, its own fixed-size instance pool, its own isolated linear-memory state. Two rules both declaring `stateful_cache(size=1024)` each maintain their own cache — merging them would leak state between rules. The `pool: N` declaration is per-call-site.

## Multi-middleware per module

One `.wasm` can export multiple middleware. Component Model supports multiple interfaces per component:

```wit
world middleware-library {
    import vane:host/host;
    export jwt-validator:   plugin;
    export session-lookup:  plugin;
}
```

Rule config references via `<module>:<middleware>`:

```json
{ "use": "auth:jwt-validator",  "args": { ... } }
{ "use": "auth:session-lookup", "args": { ... } }
```

One `.wasm` loads once; daemon reads metadata for each exported middleware; each becomes an independent FlowGraph node with its own pool.

## Observability

Plugin activity is first-class observability data:

- `log(...)` → daemon's structured log, tagged with plugin name, version, connection ID, request trace ID.
- `metric-counter` / `metric-gauge` → daemon's metrics facade, namespaced `plugin.<name>.<metric>`.
- `http-fetch` → logged with target host, outcome, latency; metrics for count, error rate, latency distribution per plugin.
- Pool events (checkout, return, exhaustion) → metrics with `plugin.<name>.pool.*` naming.

Rate limiting on plugin log emission (preventing misbehaving plugins from flooding logs) is architecturally positioned as a rate limiter at the emission point; deferred to post-MVP.
