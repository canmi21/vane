# Middleware

Middleware is the user-authored (internal Rust or external WASM) logic that runs inside a FlowGraph, between accept and terminate. Middleware reads and modifies state; it does not contact upstreams (that is Fetch's role) and does not write to the client (that is Terminator's role).

## Taxonomy

Two orthogonal axes:

- **Origin** — internal (Rust, compiled into `vaned`) or external (WASM, loaded at runtime).
- **State** — stateless (pure function of input; no observable continuity across calls) or stateful (holds state across invocations).

All four combinations are first-class:

|          | Stateless                             | Stateful                                      |
| -------- | ------------------------------------- | --------------------------------------------- |
| Internal | SNI match, path prefix, header equals | Rate limit, connection counter, cached lookup |
| External | Pure WASM predicate                   | WASM with in-memory state                     |

## Traits

Four traits, one per input scope. Each signature _is_ the inspection declaration — a middleware's parameter type communicates exactly what it can touch. Every trait takes two context parameters: `conn: &Arc<ConnContext>` (shared, mostly-immutable connection state) and `ctx: &mut FlowCtx<'_>` (per-execution mutable state; see `03-types.md`).

```rust
#[trait_variant::make(L4PeekMiddleware: Send)]
pub trait L4PeekMiddlewareLocal {
    async fn run(
        &self,
        peek: &[u8],
        conn: &Arc<ConnContext>,
        ctx:  &mut FlowCtx<'_>,
    ) -> Result<Decision, Error>;
}

#[trait_variant::make(L4BytesMiddleware: Send)]
pub trait L4BytesMiddlewareLocal {
    async fn run(
        &self,
        l4:   &mut L4Conn,
        conn: &Arc<ConnContext>,
        ctx:  &mut FlowCtx<'_>,
    ) -> Result<Decision, Error>;
}

#[trait_variant::make(L7RequestMiddleware: Send)]
pub trait L7RequestMiddlewareLocal {
    async fn run(
        &self,
        req:  &mut Request,
        conn: &Arc<ConnContext>,
        ctx:  &mut FlowCtx<'_>,
    ) -> Result<Decision, Error>;
    /// Declared body access. Drives the LazyBuffer compile-time decision (request side).
    fn needs_body(&self) -> bool { false }
}

#[trait_variant::make(L7ResponseMiddleware: Send)]
pub trait L7ResponseMiddlewareLocal {
    async fn run(
        &self,
        resp: &mut Response,
        conn: &Arc<ConnContext>,
        ctx:  &mut FlowCtx<'_>,
    ) -> Result<Decision, Error>;
    /// Declared body access. Drives the LazyBuffer compile-time decision (response side).
    fn needs_body(&self) -> bool { false }
}

pub enum Decision {
    Continue,
    Short(ShortCircuit),
}

pub enum ShortCircuit {
    Response(Response),    // L7 only: return this response to the client
    Close(CloseReason),    // L4 or L7: close the connection
}

pub enum MiddlewareInst {
    L4Peek     (Arc<dyn L4PeekMiddleware>),
    L4Bytes    (Arc<dyn L4BytesMiddleware>),
    L7Request  (Arc<dyn L7RequestMiddleware>),
    L7Response (Arc<dyn L7ResponseMiddleware>),
    Wasm       (WasmMiddleware),
}
```

### Why `L7ResponseMiddleware` does not receive `&Request`

`L7Fetch::fetch` consumes the `Request` by value (see `05-terminator.md`); after Fetch the Request no longer exists in the executor. Middleware that needs request-derived information on the response side must stash it during the request phase — typical patterns:

- Put a typed entry into `ConnContext.user` from `L7RequestMiddleware::run`, read it in `L7ResponseMiddleware::run`.
- The `HttpProxy` Fetch propagates selected request extensions onto the Response's extensions before returning (implementation detail of the Fetch).

The shape "response middleware reads consumed request" is intentionally not supported: it would require either `Arc<Request>` (forbidding body mutation) or Fetch returning the Request back out (each Fetch variant has a different story). Both tradeoffs are worse than the stash pattern.

### Why four traits instead of one with a phase tag

The trait signature makes phase violations a compile error, not a runtime bug. An `L7ResponseMiddleware`'s `run` takes `&mut Response`; it cannot accidentally reach a Request that is not there. An `L4PeekMiddleware` receives `&[u8]`; it cannot mutate a TcpStream that does not exist in its signature.

### Why `dyn Trait` storage

vtable dispatch on a middleware call is ~1 ns; middleware work (regex match, KV lookup, hashmap ops) is 100× to 10000× more. Dispatch cost is not measurable. `dyn Trait` is type-safe — unlike the rejected `dyn Any`, method signatures stay enforced at the call site.

### Async `Send` via `trait_variant`

Rust 1.95's `async fn` in traits (AFIT) is stable, but the returned future's `Send`-ness is **not** automatically part of the trait contract. Middleware impls that accidentally `await` a non-`Send` value yield a non-`Send` future — which the executor (running in tokio's multi-threaded runtime under `tokio::spawn`) cannot accept. Without a guard, the error surfaces far from the trait definition, in some downstream executor site.

We use [`trait_variant`](https://crates.io/crates/trait-variant) to generate paired traits: one local, one `Send`-bounded. Authors implement the local version; the executor stores and calls the `Send` version.

```rust
// Declaration produces two traits:
//   - L4PeekMiddlewareLocal    (no Send requirement on futures)
//   - L4PeekMiddleware          (Send-bounded; inherits from Local)
#[trait_variant::make(L4PeekMiddleware: Send)]
pub trait L4PeekMiddlewareLocal {
    async fn run(
        &self,
        peek: &[u8],
        conn: &Arc<ConnContext>,
        ctx:  &mut FlowCtx<'_>,
    ) -> Result<Decision, Error>;
}

// Authors implement Local:
impl L4PeekMiddlewareLocal for SniMatch { ... }

// MiddlewareInst stores the Send-bounded trait object:
pub enum MiddlewareInst {
    L4Peek(Arc<dyn L4PeekMiddleware>),   // Send-bounded
    ...
}
```

The compiler checks at the impl site: if the impl's future happens to be `Send` (which it will be for any middleware using our standard types defined in `vane-core`), it automatically also satisfies `L4PeekMiddleware`. If someone writes a middleware holding a non-`Send` type across an `.await`, compilation fails at their impl with a clear error, not deep inside the executor.

Zero runtime overhead — `trait_variant` uses RPITIT (return-position impl Trait in trait) under the hood, not `Box<dyn Future>`.

Dependency: `trait-variant` added to `[workspace.dependencies]` in the root `Cargo.toml`.

## Phase placement

A middleware's trait determines where it can appear in the FlowGraph. The compiler enforces this at load time (see `02-flow.md`):

- `L4PeekMiddleware`, `L4BytesMiddleware` — before Fetch, on L4 paths.
- `L7RequestMiddleware` — after L4→L7 upgrade, before Fetch.
- `L7ResponseMiddleware` — after Fetch, before `Terminator::WriteHttpResponse`. Only valid on paths ending in `WriteHttpResponse`; paths ending in `ByteTunnel` (WebSocket and L4Forward) have no response phase.

## Context parameters

Middleware receives the two context objects defined in `03-types.md`:

- `conn: &Arc<ConnContext>` — per-connection shared state (transport, TLS info, user extensions)
- `ctx:  &mut FlowCtx<'_>` — per-execution mutable state (graph ref, tracing span, flow-log sink, cancel token)

Middleware can:

- Read all fields of `ConnContext`.
- Write to `ConnContext.user` (the typed anymap). Downstream middleware reads by type.
- Enter tracing spans via `ctx.span` and emit structured events via `ctx.log`.
- Observe `ctx.cancel` to cooperate with client-disconnect / management-cancel signals.
- Not access other middleware's internal state. Cross-middleware communication is exclusively through `ConnContext.user`.
- Not touch client or upstream sockets. Those belong to Fetch and Terminator.

## Internal middleware

Built into `vaned`. Written in Rust, typed against the crate's types.

### Stateless internal

Unit structs or plain `struct(args)`. Examples:

- SNI match (reads `tls.sni`)
- Host header match (reads `http.header.host`)
- Path prefix (reads `http.uri.path`)
- Method match
- Protocol detect (reads L4 peek buffer)
- `forward_client_ip` — sets `X-Forwarded-For` (append) or `X-Real-IP` (overwrite) on the outgoing request, derived from `ConnContext.remote`. Default header set `["x-forwarded-for", "x-real-ip"]`, append mode for XFF, overwrite mode for X-Real-IP. Disabled by default at the raw-rule layer; `reverse_proxy` preset enables it automatically.

No allocation per invocation. **Hash-consed**: the `lower` pass keys by `(name, canonical_args_json)`; two rules declaring the same `path_prefix "/api"` share a single `MiddlewareId`. See `02-flow.md`'s hash-consing section.

### Stateful internal

Structs with interior mutability (`parking_lot::Mutex`, `ArcSwap`, atomics, `DashMap`). Examples:

- Rate limit (token bucket keyed by IP or other ConnContext field)
- Connection counter
- Request-ID generator

One instance per middleware-in-FlowGraph, **per call site**. Two rules both declaring `rate_limit(rate=100)` get **two distinct** `MiddlewareId`s and **two separate** buckets — silently merging them would halve the effective rate across the two rules. The `MiddlewareRegistry`'s construction path checks statefulness and disables dedup for this kind.

Lifetime is tied to the `FlowGraph`'s `Arc` — replaced when the graph swaps. Every reload therefore resets all graph-scoped stateful state (buckets empty, counters at zero, caches cleared). This is the final design — see "State migration on reload" below.

## State migration on reload

**Intentionally none.** When a config change recompiles the graph and `ArcSwap` installs it, the old `Arc<FlowGraph>` eventually drops and all its `MiddlewareInst`s drop with it; the new graph's stateful middleware are constructed fresh with empty state:

| Middleware kind                                 | Reload effect                                                                                        |
| ----------------------------------------------- | ---------------------------------------------------------------------------------------------------- |
| Internal stateless (`sni_match`, `path_prefix`) | None — no state to preserve                                                                          |
| Internal stateful (`rate_limit`, counters)      | **State resets**: buckets refill to capacity, counters return to zero                                |
| External stateless WASM                         | Instance pool drains naturally; new pool starts fresh                                                |
| External stateful WASM                          | **Linear memory resets**: old pool drains with the old graph, new pool pre-allocates fresh instances |

### Why this is the design, not a compromise

Preserving arbitrary Rust / WASM state across `ArcSwap` requires one of:

- **Identity-based migration** — every stateful `use` gets a stable `id`, the `lower` pass queries the old graph and transplants `Arc`s by identity. This is plausible but adds a formal identity layer to every middleware invocation and a non-pure stage to the compile pipeline — non-trivial complexity for a feature few rules actually need.
- **State externalization** — state lives in a KV store (sled / redis) and every call round-trips. A per-request network / disk hit breaks the proxy's hot-path budget and drags in a database dependency.

Neither fits vane's posture as a **small, fast, fully in-memory proxy whose HMR contract is "in-flight connections see no disruption"**. Seamless state preservation across config changes is explicitly **not** part of that contract.

### What to do if you need state that survives reloads

The correct place to put such state is not inside vane:

- **Put a dedicated rate-limit / flow-control layer between vane and your origin.** A separate limiter service, or a sidecar like haproxy / envoy configured in front of the origin. That layer's state is independent of vane's graph lifecycle; vane forwards bytes, the limiter owns the counters.
- **Push enforcement into the application.** The origin service carries its own per-user / per-IP limits (application middleware, a shared redis cache, a CDN-level WAF). vane stays out of the state business; application semantics are unaffected by vane reloads.
- **For coarse DDoS-class protection that is intended to survive reloads**, use the daemon-level L1 floor (see `13-rate-limit.md` — `max_conn_per_ip`, `max_total_connections`, etc.). L1 state lives on the daemon, not the graph; `ArcSwap` does not touch it. L1 is deliberately coarse and is not a substitute for application-level flow control — but for "keep the daemon alive under a SYN flood", it is the right layer and it does persist.

## External middleware (WASM)

Loaded from `.wasm` files under `/etc/vaned/wasm/`. Runtime detail in `11-wasm.md`; middleware-visible semantics summarized here.

### Stateless WASM

Declared `"stateless": true` in the rule. Backed by wasmtime's `PoolingAllocator` with a small per-instance budget. Instances are rented per call, reset, returned. No state persists between calls.

### Stateful WASM

Declared `"stateless": false` with `"pool": N` (N ≥ 1).

- N `Instance`s pre-allocated at module load.
- Per call: check out, invoke, return to the pool with linear-memory state intact.
- Pool exhausted → drop the caller connection with overload. Queueing is deliberately not implemented — unbounded queues under sustained overload produce worse failure modes than fast drops.

Pool sizing is the operator's responsibility. Auto-scaling is deferred post-MVP.

### `WasmMiddleware` shape

The struct embedded in the FlowGraph for every WASM middleware invocation site:

```rust
pub struct WasmMiddleware {
    pub module_id: ModuleId,              // index into the daemon's module registry
    pub runtime:   Arc<dyn WasmRuntime>,  // trait defined in vane-core; impl lives in vane-wasm
    pub args:      serde_json::Value,     // per-rule args passed to the plugin at invocation
    pub metadata:  Arc<PluginMetadata>,   // cached get-metadata() result; drives compile-time decisions
}

pub struct ModuleId(pub u32);

pub struct PluginMetadata {
    pub name:       String,
    pub version:    String,
    pub kind:       MiddlewareKind,       // L4Peek | L4Bytes | L7Request | L7Response
    pub stateless:  bool,
    pub needs_body: bool,
    pub inspects:   Vec<String>,          // field paths the plugin reads
}
```

Runtime dispatch: when the executor hits a `MiddlewareInst::Wasm(w)`, it calls `w.runtime.invoke(w.module_id, w.args.clone(), /* serialized ctx + input */)`. The WasmRuntime trait's implementation (in `vane-wasm`) handles pool checkout, marshaling, and result decoding — see `11-wasm.md`.

### Mode choice is user-declared

The user explicitly sets `stateless: true | false`. The runtime does not infer. The two modes have different ABI guarantees (stateless cannot observe globals across calls; stateful can), so the mode is a contract the user opts into.

## Body access and LazyBuffer

`L7RequestMiddleware` and `L7ResponseMiddleware` declare body access via `needs_body()`. The compiler consumes this declaration during `analyze` and `lower` — **as two independent tracks, one per body side** (see `02-flow.md`'s LazyBuffer section):

- The **request-side** track marks a path as request-buffered if any `L7RequestMiddleware::needs_body()` returns `true`, any `Check` reads `http.body`, or the path's Fetch has retry enabled.
- The **response-side** track marks a path as response-buffered if any `L7ResponseMiddleware::needs_body()` returns `true` (response-side field paths are future work).

A path can be request-buffered and response-streaming (or any other combination of the two). The executor collects only the side(s) flagged, replacing `Body::Http12 / Http3 / Stream` with `Body::Static(Bytes)` at the compile-time-decided trigger node. Middleware downstream of the trigger on that side always observes complete, replayable bytes.

Retry in Fetch implicitly forces request-body buffering (retry requires replay; `Body::Http12` and `Body::Http3` are one-shot streams). Only `Body::Static` and `Body::Empty` are retry-safe. A rule opting into retry cannot avoid the memory cost on the request side; the response side is unaffected.

WASM middleware declares `needs_body` in its module metadata, read at module load. Plugin metadata distinguishes request-side and response-side body needs via the middleware's `kind` (see `WasmMiddleware` below).

## Lifecycle

1. **Compile** — FlowGraph compilation resolves middleware references. Undefined references are compile errors.
2. **Instantiate** — stateless internal middleware are unit values; stateful internal middleware are constructed per graph-instance; WASM modules are compiled (or loaded from `.cwasm` cache); stateful WASM pools are pre-allocated.
3. **Execute** — per-connection invocations route via the graph. Pool checkout, reset, return are handled by the middleware runtime.
4. **Reload** — on FlowGraph swap, middleware instances referenced by the new graph are constructed; instances referenced only by the old graph drop when the old graph's `Arc` refcount reaches zero (when in-flight requests complete).

No user code runs on the reload critical path.

## Two error channels, not one

Middleware returns `Result<Decision, Error>`. The two channels have different meanings and different routing:

- **`Ok(Decision::Short(_))`** — the middleware **intentionally** refuses the request / closes the connection. This is an application-level decision: "this JWT is expired", "this IP is over quota", "this path is banned". The middleware has done its job correctly; the `Short` is the outcome it was built to produce. Routing is fixed by the enum variant: `Short::Response(r)` writes `r` to the client; `Short::Close(reason)` closes the connection.
- **`Err(Error)`** — the middleware **failed to execute**. A plugin trapped, a parse step hit an unreachable branch, a pool is exhausted, an upstream lookup the middleware depends on is unreachable. This is an internal anomaly, not a designed-in outcome. Routing follows `Node::Middleware.on_error` (see `02-flow.md`):

  | `on_error` value                       | Behavior when `run()` returns `Err(_)`                                 |
  | -------------------------------------- | ---------------------------------------------------------------------- |
  | `None` (default — fail-safe tombstone) | L7 path: synthesize `500 Internal Server Error`. L4 path: close (RST). |
  | `Some(NodeId)` (user-configured)       | Jump to target node; request continues there.                          |

The executor logs `Err(_)` to the flow log with the middleware's name and `Error::kind()` before routing, regardless of `on_error` choice.

### Why the split matters

Users writing rules often want to branch on "JWT invalid" differently from "JWT validator crashed". Collapsing both into a single error channel forces awkward config: either every failure goes to the same fallback, or the rule has to distinguish via opaque error codes. The explicit split makes "what do you want when your logic says no" (use `Decision::Short`, deterministic) distinct from "what do you want when the middleware itself is sick" (use `on_error`, rare, a safety net).

Internal stateless middleware can still produce `Err` — e.g., `SniMatch` fed a malformed ClientHello, `PathPrefix` fed a URI with invalid percent-encoding. The probability rises across the four categories: internal stateless (low), internal stateful (moderate), external stateless WASM (higher — traps, instance exhaustion, memory budget), external stateful WASM (highest — the stateful pool adds another failure mode).

### Config form

In a rule, `on_error` is declared alongside the middleware `use`:

```jsonc
// Default (no on_error) → fail-safe tombstone
{ "use": { "name": "jwt_validator" } }

// Close on failure
{ "use": { "name": "jwt_validator", "on_error": "close" } }

// Fallback to a synth response
{ "use": { "name": "jwt_validator", "on_error": { "response": { "status": 503, "body": "maintenance" } } } }
```

The `lower` pass resolves each `on_error` form into a concrete `NodeId`:

- `"close"` → a small inline subgraph ending in `Terminate(ByteTunnel-close-only)` for L4, or `Terminate(WriteHttpResponse with 5xx)` for L7.
- `{ "response": ... }` → a `Fetch::HttpSynthesize` + `Terminate(WriteHttpResponse)` subgraph.

Post-MVP: `on_error: "<rule-name>"` to reroute through another rule's entry. Deliberately excluded from MVP because inter-rule graph coupling (one rule's `on_error` targeting another's internals) complicates reasoning; single-rule self-contained fallbacks cover the common cases.

### Middleware does not retry

Retry lives inside the Fetch — see `05-terminator.md` (Retry subsection) and `07-l7.md` for the policy. Middleware is not the right layer: it cannot replay side-effects it has already committed (e.g., rate-limit counter increment).

## Non-goals

- No middleware composition language (pipe, fan-out) at the trait layer. Composition is the FlowGraph's job.
- No middleware-side rule validation. Rules are validated at compile; middleware runs pre-validated inputs.
- No thread-local side channels between middleware invocations. Cross-request state is `ConnContext.user` or the stateful middleware's own storage.
