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

Four traits, one per input scope. Each signature _is_ the inspection declaration — a middleware's parameter type communicates exactly what it can touch.

```rust
#[trait_variant::make(L4PeekMiddleware: Send)]
pub trait L4PeekMiddlewareLocal {
    async fn run(&self, peek: &[u8], ctx: &mut Ctx<'_>) -> Result<Decision, Error>;
}

#[trait_variant::make(L4BytesMiddleware: Send)]
pub trait L4BytesMiddlewareLocal {
    async fn run(&self, conn: &mut L4Conn, ctx: &mut Ctx<'_>) -> Result<Decision, Error>;
}

#[trait_variant::make(L7RequestMiddleware: Send)]
pub trait L7RequestMiddlewareLocal {
    async fn run(&self, req: &mut Request, ctx: &mut Ctx<'_>) -> Result<Decision, Error>;
    /// Declared body access. Drives the LazyBuffer compile-time decision.
    fn needs_body(&self) -> bool { false }
}

#[trait_variant::make(L7ResponseMiddleware: Send)]
pub trait L7ResponseMiddlewareLocal {
    async fn run(&self, resp: &mut Response, ctx: &mut Ctx<'_>) -> Result<Decision, Error>;
    /// Declared body access. Drives the LazyBuffer compile-time decision.
    fn needs_body(&self) -> bool { false }
}

pub enum Decision {
    Continue,
    Short(ShortCircuit),
    Branch(BranchId),
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
    async fn run(&self, peek: &[u8], ctx: &mut Ctx<'_>) -> Result<Decision, Error>;
}

// Authors implement Local:
impl L4PeekMiddlewareLocal for SniMatch { ... }

// MiddlewareInst stores the Send-bounded trait object:
pub enum MiddlewareInst {
    L4Peek(Arc<dyn L4PeekMiddleware>),   // Send-bounded
    ...
}
```

The compiler checks at the impl site: if the impl's future happens to be `Send` (which it will be for any middleware using our `Ctx` and the standard types defined in `vane-core`), it automatically also satisfies `L4PeekMiddleware`. If someone writes a middleware holding a non-`Send` type across an `.await`, compilation fails at their impl with a clear error, not deep inside the executor.

Zero runtime overhead — `trait_variant` uses RPITIT (return-position impl Trait in trait) under the hood, not `Box<dyn Future>`.

Dependency: `trait-variant` added to `[workspace.dependencies]` in the root `Cargo.toml`.

## Phase placement

A middleware's trait determines where it can appear in the FlowGraph. The compiler enforces this at load time (see `02-flow.md`):

- `L4PeekMiddleware`, `L4BytesMiddleware` — before Fetch, on L4 paths.
- `L7RequestMiddleware` — after L4→L7 upgrade, before Fetch.
- `L7ResponseMiddleware` — after Fetch, before `Terminator::WriteHttpResponse`. Only valid on paths ending in `WriteHttpResponse`; paths ending in `ByteTunnel` (WebSocket and L4Forward) have no response phase.

## Context

```rust
pub struct Ctx<'a> {
    pub conn:  &'a Arc<ConnContext>,    // connection-level, shared across every middleware on this conn
    pub graph: &'a FlowGraph,           // read-only, for sibling lookups
    // future: tracing span, flow-log sink, cancellation token
}
```

Middleware can:

- Read all fields of `ConnContext`.
- Write to `ConnContext.user` (the typed anymap). Downstream middleware reads by type.
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

No allocation per invocation.

### Stateful internal

Structs with interior mutability (`parking_lot::Mutex`, `ArcSwap`, atomics, `DashMap`). Examples:

- Rate limit (token bucket keyed by IP or other ConnContext field)
- Connection counter
- Request-ID generator

One instance per middleware-in-FlowGraph. Lifetime is tied to the `FlowGraph`'s `Arc` — replaced when the graph swaps.

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

`L7RequestMiddleware` and `L7ResponseMiddleware` declare body access via `needs_body()`. The compiler consumes this declaration during `analyze` and `lower`:

- If any reachable middleware on a path declares `needs_body() == true`, **or** the path's Fetch has retry enabled, the body on that path is **eagerly buffered**: the runtime collects all frames (including trailers) into `Body::Static(Bytes)` before invoking any middleware that touches it.
- Otherwise, the body streams through as `Bytes` chunks with zero intermediate copies.

Eager means middleware always sees complete, replayable bytes — never a half-arrived stream. This trades startup latency (on paths the user explicitly asked for body inspection) for semantic simplicity.

Retry in Fetch implicitly forces request-body buffering. Retry requires replay; `Body::Http12` and `Body::Http3` are one-shot streams that cannot be re-polled. Only `Body::Static` and `Body::Empty` are retry-safe. A rule opting into retry cannot avoid the memory cost.

WASM middleware declares `needs_body` in its module metadata, read at module load.

## Lifecycle

1. **Compile** — FlowGraph compilation resolves middleware references. Undefined references are compile errors.
2. **Instantiate** — stateless internal middleware are unit values; stateful internal middleware are constructed per graph-instance; WASM modules are compiled (or loaded from `.cwasm` cache); stateful WASM pools are pre-allocated.
3. **Execute** — per-connection invocations route via the graph. Pool checkout, reset, return are handled by the middleware runtime.
4. **Reload** — on FlowGraph swap, middleware instances referenced by the new graph are constructed; instances referenced only by the old graph drop when the old graph's `Arc` refcount reaches zero (when in-flight requests complete).

No user code runs on the reload critical path.

## Error propagation

Middleware returns `Result<Decision, Error>`. An `Err(_)` return:

- **L4** — close the connection. Error is logged to the flow log with middleware name and `Error::kind()`.
- **L7 Request** — return a 5xx to the client (`500` by default; `Timeout → 504`; `Upstream → 502`). Skip Fetch.
- **L7 Response** — the upstream response is already obtained; a response-middleware error replaces the response with a 5xx. Upstream work is not rolled back.

Middleware does not retry. Retry lives inside the Fetch — see `05-terminator.md` (Retry subsection) and `07-l7.md` for the policy.

## Non-goals

- No middleware composition language (pipe, fan-out) at the trait layer. Composition is the FlowGraph's job.
- No middleware-side rule validation. Rules are validated at compile; middleware runs pre-validated inputs.
- No thread-local side channels between middleware invocations. Cross-request state is `ConnContext.user` or the stateful middleware's own storage.
