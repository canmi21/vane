# vane-wasm

Source: [`crates/wasm/`](../../crates/wasm/).

WASM plugin runtime. Separated from `vane-engine` so engine can build and test without wasmtime.

The wire contract that plugin authors target lives in [`../wasm-abi.md`](../wasm-abi.md). This file covers the host-side runtime: lifecycle, pools, host functions, observability.

## Owns

- `WasmtimeRuntime: vane_core::WasmRuntime`. Source: `lib.rs`.
- Component Model loading via wasmtime's `bindgen!`, `get-metadata` invocation, metadata caching.
- Instance pools — `PoolingAllocator` for stateless plugins; fixed-size pre-allocated pools for stateful.
- Host function implementations — `log`, `now-unix-ms`, `random`, `metric-counter`, `metric-gauge`, `http-fetch`.
- Per-plugin metric cardinality enforcement. Source: `cardinality.rs`.
- `inspects` capability validation — plugin-declared field paths are checked against the authoritative path table at load. Source: `inspects.rs`.

`http-fetch` routes through `vane-engine`'s `TcpPool` via the `HttpFetchBackend` trait declared in `vane-core` so `vane-wasm` does not depend on `vane-engine`. The daemon injects an `Arc<dyn HttpFetchBackend>` into `WasmtimeRuntime` before loading any plugins.

## Crate dependencies

`vane-core` + `wasmtime`, `wasmtime-wasi`, `wit-bindgen`, `bytes`.

## Runtime: wasmtime

Mature, actively developed, async-native via `Config::async_support`. `PoolingAllocator` designed for the exact instance-reuse pattern vane needs. Component Model support is stable since 2024; vane commits to Component Model as the plugin ABI.

Component Model (not bincode) keeps the polyglot promise: plugins written in Rust, Go (TinyGo), AssemblyScript, JavaScript (via QuickJS), or other WASM-targeting languages all compile to the same component format. Adding optional record fields to WIT does not break old plugins. Slightly higher per-call cost than raw bincode (typed marshaling) — negligible next to actual plugin work.

## Plugin metadata drives compilation

`registry.get-metadata()` is called once at component load. The cached metadata feeds:

- **FlowGraph compilation** — `kind` determines phase placement; `needs_body` drives LazyBuffer; `inspects` is a strict capability declaration that drives both inspection-level analysis (rule sorting, predicate sharing) and the host-side context-packing budget at call time.
- **Pool allocation** — `stateless` chooses between `PoolingAllocator` (cheap reuse) and fixed pre-allocated pool.
- **Rule validation** — a rule using a plugin as `L7RequestMiddleware` must reference a plugin whose metadata `kind == l7-request`; mismatch is a compile error.

Full metadata shape: [`../wasm-abi.md` § _Registry_](../wasm-abi.md#registry).

## Module lifecycle

```
/etc/vaned/wasm/*.wasm          source components
/var/lib/vaned/wasm/*.cwasm     wasmtime-compiled cache, keyed by content hash
```

### Boot

For each `.wasm` in the config directory:

1. Compute content hash.
2. If matching `.cwasm` exists → `Component::deserialize` (microsecond-scale).
3. Otherwise `Component::from_file` compiles (millisecond-per-KB); persist to `.cwasm`.
4. Instantiate, call `get-metadata()`, cache metadata.

### Hot reload

File watcher detects changed `.wasm`. Compile new `Component`; call `get-metadata()`.

`module_id` is the canonical absolute filesystem path. Renaming or moving a `.wasm` is treated as deletion + addition: old `module_id` drops, new one compiles into the next graph generation.

| Change                                                                           | Effect                                                                                                                                                                                                                                                                                                                                                                                                 |
| -------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Metadata unchanged (`kind`, `stateless`, `needs_body`, `inspects`, exports same) | Module-only swap. FlowGraph not recompiled. Stateless: `PoolingAllocator` transparently creates instances against the new component; in-flight stateless invocations finish on old instances and drop. Stateful: old pool continues serving in-flight checkouts; new checkouts construct against the new component. Linear memory bound to old layout does not migrate even if metadata is compatible. |
| Metadata changed                                                                 | Full FlowGraph recompile (compile decisions depend on metadata, including `inspects` which drives capability-bounded context packing). New graph ArcSwaps in. All stateful linear memory on that module resets — see [`flow-model.md` § _State migration_](../flow-model.md#state-migration-on-reload).                                                                                                |

`metadata.name` and `metadata.version` changes alone do not affect routing — they only annotate metric and log labels.

## Instance pool

Two modes, declared per plugin's `stateless` metadata:

### Stateless

- Backed by wasmtime `PoolingAllocator` with small per-instance memory budget (default 1 MiB).
- Each invocation rents an instance, runs, returns; next call gets fresh memory.
- Pool grows on demand; daemon-wide cap (default 32) applies across all stateless plugins.
- Exhaustion: drop connection with overload error.

### Stateful

- Declared with `pool: N` (N ≥ 1, default 4).
- N instances pre-allocated at module load. Each call checks out, invokes, returns — linear-memory state persists within a single graph generation.
- Pool size fixed; auto-scaling deferred.
- Exhaustion: drop connection with 503. Queueing deliberately not implemented — unbounded queues under sustained overload produce worse failure modes than fast drops.
- On FlowGraph reload (metadata changed, or any other recompile-triggering change): pool drops with the old graph. New graph pre-allocates a fresh pool of N empty-state instances. Linear memory does not migrate.

```rust
// TODO(wasm-pool-autoscale): auto-scaling for stateful pools. MVP uses
// operator-configured fixed sizes. The shape is on the operator's hands;
// auto-scaling needs a load model the project does not yet have.
```

## Host functions

Minimal whitelist (full signatures in [`../wasm-abi.md` § _Host functions_](../wasm-abi.md#host-functions)):

| Function                                         | Purpose                                                        |
| ------------------------------------------------ | -------------------------------------------------------------- |
| `get-args() -> string`                           | One-shot per-instance JSON args delivery                       |
| `log(level, message, fields)`                    | Structured log: message + key-value fields (string values)     |
| `now-unix-ms() -> u64`                           | Wall clock                                                     |
| `random(buf-len) -> list<u8>`                    | Cryptographic RNG                                              |
| `metric-counter(name, delta, labels)`            | Emit counter event with labels (host enforces cardinality cap) |
| `metric-gauge(name, value, labels)`              | Emit gauge event with labels                                   |
| `http-fetch(request) -> result<response, error>` | Outbound HTTP request via the daemon's TcpPool                 |

Not provided: network beyond `http-fetch`, filesystem, environment variables, process or thread spawn. Plugins are pure logic; external observation goes through whitelisted host functions under daemon control.

The host enforces a per-plugin metric cardinality cap (default 1000 series); excess emissions drop and a single warn-level log fires per cap event per plugin. Source: `cardinality.rs`.

### `http-fetch` policy

Wire shape: [`../wasm-abi.md` § _Host functions_](../wasm-abi.md#host-functions).

- **Full body, non-streaming.** Streaming inside a plugin is too complex for the request-response decision use case. `max-body-size` default: 1 MiB request, 1 MiB response, per-plugin configurable.
- **Failures are typed, not traps.** Plugin handles via `Result` matching — fallback, retry, cache, or fail. Traps are reserved for panic-level plugin bugs.
- **Three-level timeout fallback** — per-call `timeout-ms` → plugin config default → daemon default (30 s). Each level overrides the next; the daemon level guarantees a finite timeout always applies.
- **Redirect handling** — per-call `follow-redirects` → plugin config default (default 5 hops). `0` disables redirects. Plugin sees the final response after redirects; intermediate hops are logged at the plugin scope.
- **TLS verification is two-gate** — per-call `verify-tls: false` is honored only when the plugin config also has `allow-insecure: true`. Both must agree for verification skip; otherwise the request fails with `insecure-rejected`.
- **Shares the daemon's TcpPool** — same fingerprint, same observability as Fetch upstreams. Cross-crate wiring goes through `HttpFetchBackend` trait declared in `vane-core`; engine provides the concrete impl wrapping `TcpPool`; daemon injects at startup. Tests substitute a mock backend.
- **`allowed_hosts`** per-plugin config; default `["*"]` (no restriction). Narrow as needed (e.g. `["auth.example.com", "*.internal"]`); requests outside the list return `not-allowed`.

```rust
// TODO(wasm-rate-limit): per-plugin rate limiting (daemon-level token
// bucket keyed by plugin name) is architected but not yet implemented.
```

## Memory and time limits

| Resource      | Default | Cap                 | Configurable per plugin |
| ------------- | ------- | ------------------- | ----------------------- |
| Linear memory | 1 MiB   | 128 MiB daemon-wide | yes                     |
| Per-call time | 10 ms   | —                   | yes                     |

Time enforcement is epoch-based preemption (not fuel). Epoch has negligible steady-state overhead (checked at periodic ticks); fuel adds per-instruction cost. Wasmtime sets an epoch deadline per call; plugin work interrupts on exceeding. The host increments the epoch counter every 1 ms — combined with the 10 ms default deadline, plugin invocations are preempted within `10 ms ± 1 ms`. Tick frequency is fixed (not configurable per plugin) to keep host-side overhead constant regardless of plugin count.

The ABI does not propagate cancellation; client disconnect mid-invocation is not signaled to the plugin. The 10 ms ceiling makes proactive cancellation a marginal optimization. See [`../wasm-abi.md` § _Cancellation_](../wasm-abi.md#cancellation) for the forward-compatibility note.

## Trap and error handling

| Condition                                   | Handling                                                                                                                                                                                                                                                                                                                      |
| ------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Trap (OOB access, divide-by-zero, overflow) | Log plugin name + module hash + wasm stack (if available); failure                                                                                                                                                                                                                                                            |
| Memory budget exceeded                      | Same                                                                                                                                                                                                                                                                                                                          |
| Time budget exceeded (epoch preempt)        | Same                                                                                                                                                                                                                                                                                                                          |
| Malformed output (WIT decode fail)          | Same                                                                                                                                                                                                                                                                                                                          |
| Component returns `Err(plugin-error)`       | **Not a trap** — plugin-originated structured error `{ code, message, on-error-hint }`; flows through the regular middleware error channel. `on-error-hint` chooses between rule-config `on_error`, `force-close`, and `internal` semantics — full table in [`../wasm-abi.md` § _plugin-error_](../wasm-abi.md#plugin-error). |

On any of the above, the plugin invocation returns `Err(Error)` up through the middleware trait method. From there the standard middleware error channel takes over — same mechanism that handles non-WASM middleware errors. See [`flow-model.md` § _Two error channels_](../flow-model.md#two-error-channels).

## Dedup policy

WASM plugin kinds follow the middleware dedup policy in [`flow-model.md` § _Hash-consing_](../flow-model.md#hash-consing).

- Stateless WASM (`stateless: true`): hash-consed by `(module_id, export_name, canonical_args_json)`. Two rules invoking the same export with the same args share one `MiddlewareId`. The runtime's `PoolingAllocator` reuses Instances across the shared `MiddlewareId` invocations.
- Stateful WASM (`stateless: false`): never deduped. Each call site gets its own `MiddlewareId`, its own fixed-size pool, its own isolated linear-memory state. Two rules both declaring `stateful_cache(size=1024)` each maintain their own cache — merging them would leak state between rules. The `pool: N` declaration is per-call-site.

## Multi-middleware per module

One `.wasm` can export multiple middleware. Each export is named in `metadata.exports` and dispatched within the corresponding `handler-*` interface via the `name` parameter of `handle(name, input)`. A single component may export multiple middlewares of the same kind (two `l7-request` validators) or mix kinds (one `l4-peek` plus one `l7-request`).

Rule config references via `<module>:<export-name>`:

```json
{ "use": "auth-bundle:jwt-validator",  "args": { ... } }
{ "use": "auth-bundle:session-lookup", "args": { ... } }
```

Stateless dedup is keyed on `(module_id, export_name, canonical_args_json)`; stateful exports never dedup.

## Observability

Plugin activity is first-class observability data:

- `log(...)` → daemon's structured log, tagged with plugin name, version, connection ID, request trace ID.
- `metric-counter` / `metric-gauge` → daemon's metrics facade, namespaced `plugin.<name>.<metric>`.
- `http-fetch` → logged with target host, outcome, latency; metrics for count, error rate, latency distribution per plugin.
- Pool events (checkout, return, exhaustion) → metrics with `plugin.<name>.pool.*`.

```rust
// TODO(wasm-log-rate-limit): rate limiting on plugin log emission
// (preventing misbehaving plugins from flooding logs) is architecturally
// positioned at the emission point but not yet implemented.
```
