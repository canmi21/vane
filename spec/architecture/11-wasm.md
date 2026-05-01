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

## WIT interface

The full WIT contract — every record, variant, host function, error variant, path grammar, trap condition, and the versioning policy — is locked in [`spec/wasm-abi.md`](../wasm-abi.md). Stage 3 ships `vane:plugin@0.1.0`. This section summarizes only the load-bearing shape decisions that affect runtime behavior described elsewhere in this document.

- **One handler interface per `middleware-kind`** (`handler-l4-peek`, `handler-l4-bytes`, `handler-l7-request`, `handler-l7-response`). A component exports only the kinds it implements. Within an interface, `handle(name, input)` selects which export of that kind serves the call, so a single component may export multiple middlewares per kind.
- **`registry.get-metadata()`** is the single load-time call. Returns `metadata { name, version, abi-version, exports: list<middleware-export> }`. The host validates `abi-version` major equals its own, that every declared `kind` has the matching `handler-*` interface exported, and that no export sets `needs-streaming-body: true` (reserved for forward compatibility).
- **`inspects` is a strict capability declaration**, not advisory. The host packs only declared field paths into the call's `context` channel; reading other paths is impossible because the data is not delivered. Path grammar mirrors `18-predicate-schema.md` and is enumerated in `wasm-abi.md` § _Context exposure_.
- **Args delivered once via `host.get-args() -> string`** at instance construction, not on every call. Args are configuration, not request data.
- **Body is `option<bytes-view> { data, truncated }`**, present iff the export declared `needs-body: true`. Default body limits: 1 MiB request, 1 MiB response, 64 KiB l4-bytes (per-plugin overridable). Streaming is not in 0.1.0.
- **Header names are ASCII-lowercase** by host guarantee. Multi-valued headers preserve wire order in the list.
- **Decision variants are kind-narrowed**: `l4-peek-decision { continue, close }`, `l4-bytes-decision { continue, tunnel, close }`, `l7-request-decision { continue, short(synth-response), close }`, `l7-response-decision { continue, modify(modified-response), abort }`. There is no plugin-driven routing variant — plugins decide, FlowGraph routes.
- **`plugin-error` is `record { code, message, on-error-hint: option<string> }`**. `on-error-hint` accepts `none`, `"force-close"`, `"internal"`. See `wasm-abi.md` § _plugin-error_.

## Plugin metadata drives compilation

`get-metadata()` is called once at module load. The cached metadata feeds:

- **FlowGraph compilation** — `kind` determines phase placement; `needs_body` drives LazyBuffer; `inspects` is a strict capability declaration that drives both inspection-level analysis (rule sorting, predicate sharing) and the host-side context-packing budget at call time.
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

`module_id` is the canonical absolute filesystem path of the `.wasm` file. Renaming or moving a `.wasm` is treated as deletion + addition: the old `module_id` drops; a new one compiles into the next graph generation.

- **Metadata unchanged** (`kind`, `stateless`, `needs_body`, `inspects`, and the exported `name` set all equal per export) — module swap only; `FlowGraph` is not recompiled. Module registry swaps the active `Component` for that `module_id`. For **stateless** plugins: new invocations rent instances from `PoolingAllocator` which transparently creates new instances against the new component; in-flight stateless invocations (if any) finish and their instances drop. For **stateful** plugins: the old instance pool continues to serve in-flight checkouts until they return naturally; new checkouts will construct against the new component. Old instances drop as they complete. (Note: stateful linear memory bound to the old component's layout does **not** migrate to the new component even if metadata is compatible — each instance's state lives only as long as that instance.) `metadata.name` and `metadata.version` changes alone do not affect routing; they only annotate metric and log labels.
- **Metadata changed** — triggers **full FlowGraph recompile** (because compilation decisions depend on metadata, including `inspects` which drives capability-bounded context packing). The new graph ArcSwaps into place; old graph drops when its in-flight requests finish. All stateful linear memory on that module resets — see `04-middleware.md` § _State migration on reload_.

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

The minimal whitelist (full signatures in `wasm-abi.md` § _Host functions_):

| Function                                         | Purpose                                                        |
| ------------------------------------------------ | -------------------------------------------------------------- |
| `get-args() -> string`                           | One-shot per-instance JSON args delivery                       |
| `log(level, message, fields)`                    | Structured log: message + key-value fields (string values)     |
| `now-unix-ms() -> u64`                           | Wall clock                                                     |
| `random(buf-len) -> list<u8>`                    | Cryptographic RNG                                              |
| `metric-counter(name, delta, labels)`            | Emit counter event with labels (host enforces cardinality cap) |
| `metric-gauge(name, value, labels)`              | Emit gauge event with labels                                   |
| `http-fetch(request) -> result<response, error>` | Outbound HTTP request via daemon's TcpPool                     |

**Not provided**: network (other than `http-fetch`), filesystem, environment variables, process/thread spawn. Plugins are pure logic; external observation goes through whitelisted host functions under daemon control.

The host enforces a per-plugin metric cardinality cap (default 1000 series); excess emissions drop and a single warn-level log fires per cap event per plugin. This prevents runaway-label plugins from poisoning the metrics backend.

## `http-fetch` policy

Wire shape (full record / variant definitions in `wasm-abi.md` § _Host functions_):

```wit
http-fetch: func(req: http-fetch-request) -> result<http-fetch-response, net-error>;
```

`http-fetch-request` carries `method`, `url`, `headers`, `body`, plus three optional per-call knobs: `timeout-ms`, `follow-redirects`, `verify-tls`. `net-error` enumerates `dns-failure`, `connection-refused`, `timeout`, `tls-error`, `pool-exhausted`, `body-too-large`, `not-allowed`, `insecure-rejected`, `internal`.

Policy:

- **Full body, non-streaming**. Streaming inside a plugin is too complex for the `http-fetch` use case (request-response for decisions). `max-body-size` default: 1 MiB request, 1 MiB response, per-plugin configurable.
- **Failures are typed, not traps**. Plugin handles via `Result` matching — decide to fallback, retry, cache, or fail. Traps are reserved for panic-level plugin bugs.
- **Three-level timeout fallback**: per-call `timeout-ms` → plugin config default → daemon default (30 s). Each level overrides the next; the daemon level guarantees a finite timeout always applies.
- **Redirect handling**: per-call `follow-redirects` → plugin config default (default 5 hops). `0` disables redirects. The plugin sees the final response after redirects; intermediate hops are logged at the plugin scope.
- **TLS verification is two-gate**: per-call `verify-tls: false` is honored only when the plugin config also has `allow-insecure: true`. Both must agree for verification to be skipped; otherwise the request fails with `insecure-rejected`. This prevents a misbehaving plugin from disabling verification a deployment did not authorize.
- **Shares the daemon's TcpPool** — same fingerprint, same pool, same observability as Fetch upstreams. Cross-crate wiring goes through a trait defined in `vane-core` so `vane-wasm` does not depend on `vane-engine`:

  ```rust
  // vane-core
  #[async_trait]
  pub trait HttpFetchBackend: Send + Sync {
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

Time enforcement: **epoch-based preemption** (not fuel). Epoch has negligible steady-state overhead (checked at periodic ticks); fuel adds per-instruction cost. Wasmtime sets an epoch deadline per call; plugin work interrupts on exceeding. The host increments the epoch counter every 1 ms — combined with the 10 ms default deadline, plugin invocations are preempted within `10 ms ± 1 ms`. The tick frequency is fixed (not configurable per plugin) to keep host-side overhead constant regardless of plugin count.

The ABI does not propagate cancellation; client disconnect mid-invocation is not signaled to the plugin, which runs to completion or hits the epoch deadline. The 10 ms ceiling makes proactive cancellation a marginal optimization. See `wasm-abi.md` § _Cancellation_ for the forward-compatibility note.

## Trap and error handling

All abnormal plugin exits, handled uniformly:

| Condition                                   | Handling                                                                                                                                                                                                                                                                                                            |
| ------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Trap (OOB access, divide-by-zero, overflow) | Log plugin name + module hash + wasm stack (if available); failure                                                                                                                                                                                                                                                  |
| Memory budget exceeded                      | Same                                                                                                                                                                                                                                                                                                                |
| Time budget exceeded (epoch preempt)        | Same                                                                                                                                                                                                                                                                                                                |
| Malformed output (WIT decode fail)          | Same                                                                                                                                                                                                                                                                                                                |
| Component returns `Err(plugin-error)`       | **Not a trap** — plugin-originated structured error `{ code, message, on-error-hint }`; flows through the regular `Middleware::Error` path (see `04-middleware.md`). `on-error-hint` chooses between rule-config `on_error`, `force-close`, and `internal` semantics; full table in `wasm-abi.md` § _plugin-error_. |

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

One `.wasm` can export multiple middleware. Each export is named in `metadata.exports` and dispatched within the corresponding `handler-*` interface via the `name` parameter of `handle(name, input)`. A single component may export multiple middlewares of the same kind (two `l7-request` validators) or mix kinds (one `l4-peek` plus one `l7-request`).

```wit
// the plugin author's world declares the kinds it implements
world middleware-library {
    import vane:host/host@0.1.0;
    export vane:plugin/registry@0.1.0;
    export vane:plugin/handler-l4-peek@0.1.0;
    export vane:plugin/handler-l7-request@0.1.0;
}
```

```rust
// metadata returned at load
metadata {
    name: "auth-bundle",
    version: "1.4.0",
    abi-version: "0.1.0",
    exports: [
        { name: "ip-allowlist",   kind: l4-peek,     ... },
        { name: "jwt-validator",  kind: l7-request,  ... },
        { name: "session-lookup", kind: l7-request,  ... },
    ],
}
```

Rule config references via `<module>:<export-name>`:

```json
{ "use": "auth-bundle:jwt-validator",  "args": { ... } }
{ "use": "auth-bundle:session-lookup", "args": { ... } }
```

One `.wasm` loads once; the daemon reads metadata for each declared export; each becomes an independent FlowGraph node with its own pool. Stateless dedup is keyed on `(module_id, export_name, canonical_args_json)`; stateful exports never dedup.

## Observability

Plugin activity is first-class observability data:

- `log(...)` → daemon's structured log, tagged with plugin name, version, connection ID, request trace ID.
- `metric-counter` / `metric-gauge` → daemon's metrics facade, namespaced `plugin.<name>.<metric>`.
- `http-fetch` → logged with target host, outcome, latency; metrics for count, error rate, latency distribution per plugin.
- Pool events (checkout, return, exhaustion) → metrics with `plugin.<name>.pool.*` naming.

Rate limiting on plugin log emission (preventing misbehaving plugins from flooding logs) is architecturally positioned as a rate limiter at the emission point; deferred to post-MVP.
