# Flow model

The Flow model is vane's central abstraction. Every other doc serves it.

## The funnel

Vane is a single-trunk funnel. Every byte entering `vaned` — TCP stream, UDP datagram, decoded HTTP request — enters one dispatch engine. That engine walks a single immutable `FlowGraph`.

Per-protocol pipelines fail because "what protocol is this" and "what does the operator want done" are independent axes. Encoding both as parallel code paths forces crosscut logic ("HTTPS and SSH both on `:443`") into special-case glue. The funnel inverts this: the dispatch engine is one thing; decisions live in the graph.

## Rules

Operators write rules, not graphs:

```json
{
  "rule": "<unique-name>",
  "listen": ["<port-or-address>", ...],
  "match": [ <predicate>, ... ],
  "terminate": { <terminator-config> }
}
```

- `rule` — globally unique; used for conflict resolution, log attribution, metrics.
- `listen` — ports or addresses this rule applies to.
- `match` — zero or more predicates; all must hold (zero = always match).
- `terminate` — see [`crates/engine.md` § _Fetch and Terminator_](crates/engine.md#fetch-and-terminator).

A predicate reads a field from the connection context (`transport`, `tls.sni`, `http.header.host`, `http.body`, …) and applies an operator. It does not name hooks; the compiler derives required hooks from predicate field access.

Predicate JSON shape, field-path grammar, and operator matrix: [`crates/core.md` § _Predicate_](crates/core.md#predicate).

## Compile and link — two stages, two crates

Config becomes an executable graph in two phases:

```
[vane-core]                    no hyper / rustls / wasmtime / tokio runtime
RuleSet
  ↓ merge       dedup rule names, resolve order, emit conflict log
MergedConfig
  ↓ expand      preset expansion → RawRules
RawRuleSet
  ↓ analyze     inspection level, specificity, LazyBuffer tracks
AnalyzedRuleSet
  ↓ lower       group by listener, sort, build tree, flatten, hash-cons
SymbolicFlowGraph
  ↓ validate    NodeId resolution, DAG, phase machine, predicate-field legality
Arc<SymbolicFlowGraph>

[vane-engine]                  runtime; holds hyper / rustls / wasmtime / tokio
Arc<SymbolicFlowGraph>
  ↓ link        resolve symbolic refs via factory registries; feature-availability rejection;
                instantiate trait objects
Arc<FlowGraph>                 ArcSwap target; the executor reads this
```

Source: `crates/core/src/compile/{merge,expand,analyze,lower,validate}.rs`, `crates/engine/src/flow_graph.rs::link`.

`SymbolicFlowGraph` is pure IR (no trait objects, JSON-serializable). `FlowGraph` is the linked form holding `Vec<MiddlewareInst>` and `Vec<FetchInst>` of `Arc<dyn Trait>`; it only exists in engine.

- `vane compile <DIR>` and `vane lint` link only `vane-core` — they produce `SymbolicFlowGraph` and serialize as JSON. No hyper, no wasmtime, no tokio runtime.
- `vaned` boot and reload run both stages.

Both Arcs are cheap to swap; only the linked `FlowGraph` is `ArcSwap`-managed at runtime. Each stage's input fully determines its output; stages are independently testable.

## The compiled form

Both graphs share a flat, index-based shape. Nodes, predicate instances, fetches, middleware, and terminator slots live in parallel `Vec`s; references are typed newtype indices (`NodeId`, `PredicateId`, `MiddlewareId`, `FetchId`, `TerminatorId`).

Concrete shapes: `crates/core/src/ir.rs` (`SymbolicFlowGraph`, `Node`, ID newtypes, `FlowGraphMeta`); `crates/engine/src/flow_graph.rs` (`FlowGraph` linked form).

Why flat:

- **Cache locality** — `Node`s are contiguous; the prefetcher loads adjacent nodes.
- **Subtree sharing** — two rules compiling to the same subgraph share nodes via shared indices.
- **Stable serialization** — flat form dumps as `{ nodes: [...], entries: { ... }, ... }`, diff-friendly JSON.
- **Single allocation** — `Vec::with_capacity` then grow; not N `Box::new` calls.

`Node::Upgrade` is the explicit L4→L7 phase boundary inserted by `lower` on every L7 path. The node carries no parameters — protocol-stack initialization (TLS handshake, ALPN dispatch, HTTP version selection) is driven by the listener config attached to `Arc<ConnContext>` at runtime.

`Node::Middleware.on_error` routes `Err(Error)` returns. Default (`None`) is the fail-safe tombstone: L7 path writes `500 Internal Server Error`; L4 path closes with RST. `Some(NodeId)` jumps to the named target. Config-level `on_error: "close"` and `on_error: { "response": ... }` are lower-resolved into concrete subgraphs. The application-level refusal channel (`Decision::Short`) is a separate concern — see § _Two error channels_ below.

## Phase state machine

Every position in a compiled graph belongs to exactly one phase:

```rust
pub enum Phase {
    L4Raw,      // pre-peek: TCP/UDP socket exists, no bytes read, no TLS
    L4Peeked,   // PeekResult.buffer populated; TLS ClientHello may be parsed
    L7Request,  // Request decoded from HTTP; entering request middleware
    L7Response, // Response produced by Fetch; entering response middleware
    Tunnel,     // byte-bidirectional forwarding handed to Terminator::ByteTunnel
}
```

Source: `crates/core/src/phase.rs`.

The transition table (accepted in-phases, out-phase per node kind) is the authoritative contract that `validate` enforces and the four middleware traits make compile-checkable. The validator DFS-walks each entry starting at `L4Raw`, looking up each node in the table. Violations name the offending node, the source rule, and the expected vs actual phase.

Transition table reference: `crates/core/src/phase.rs::Transition`.

`Terminate(Close)` is phase-agnostic — `lower` may emit it on an L4 path (no TCP rule matched → RST), on an L7 path before `Upgrade` (L4 predicates all missed), or after `Upgrade` (HTTP request decoded but no rule matched).

## Executor

Iterative walker. A single `async fn` holds a loop; the loop walks the flat graph by updating a `NodeId` cursor and maintaining per-phase owned state slots (`l4`, `req`, `resp`, `tunnel`). Source: `crates/engine/src/executor.rs`.

Why iterative, not recursive: the entire execution is one `Future`, one state machine, one allocation per request. Recursive `async fn` requires `Box::pin` per call; at 10k QPS × 10 nodes per request that is 100k saved allocations per second.

`execute` returns `ExecutorOutput`:

- `Closed` — Terminator::Close walked, or any path the executor finalised without producing a response or tunnel. Listener drop-glue closes the underlying transport.
- `HttpResponse(Response)` — Terminator::WriteHttpResponse walked. The hyper service-fn returns this from H1 (and later H2 / H3) handlers.
- `Tunneled` — Terminator::ByteTunnel walked. The executor already drove `tokio::io::copy_bidirectional` raced against `ctx.cancel`. Close reason was sent through `Tunnel.close_reason_tx`.

Ownership is type-system enforced — every owned resource has exactly one consumer:

| Resource   | Created by                                              | Consumed by                                                      |
| ---------- | ------------------------------------------------------- | ---------------------------------------------------------------- |
| `L4Conn`   | accept loop                                             | `L4Fetch::fetch` (moved into `Tunnel.upstream`) OR L4→L7 upgrade |
| `Request`  | L4→L7 upgrade (HTTP decoder)                            | `L7Fetch::fetch` OR dropped on `Decision::Short(Response)`       |
| `Response` | `L7Fetch::fetch` OR L7-request middleware short-circuit | `Terminator::WriteHttpResponse`                                  |
| `Tunnel`   | `L4Fetch::fetch` OR `L7Fetch::fetch` (WS-101)           | `Terminator::ByteTunnel`                                         |

### `Terminator::Close` — wire-level manifestation

| Where the executor returned                                | Wire behaviour                                                                     |
| ---------------------------------------------------------- | ---------------------------------------------------------------------------------- |
| L4 path (back to listener accept loop)                     | TCP RST / QUIC stream reset / Unix shutdown. No HTTP framing.                      |
| L7 path inside `drive_h1_server` (an H1 request hit Close) | Synthesised status response, `Connection: close`. **404** on H1, **421** on H2/H3. |

The L7 status is a proxy-layer "no route" signal — vane is a gateway, so 421 ("server is not configured to produce responses for this URI") is RFC-accurate. 404 is the H1 fallback because RFC 9110 introduced 421 alongside H2 and H1-only clients may handle 421 oddly. The 5xx codes (502/503) read as "tried something downstream and it failed" — wrong here, no upstream was contacted.

### `tracing` integration

One event per loop iteration: `trace!(node_id = ?cur, kind = "check" | "mid" | "fetch" | "terminate")`. This is the per-step trace; `RUST_LOG` gates it. The structured `FlowLogSink` stream is independent — see § _Flow log_ below.

## LazyBuffer

Bodies stream by default. They are buffered only where a node on the path actually needs replayable bytes. Request-side and response-side are two independent tracks — a path can be request-buffered and response-streaming, or any combination.

For each path from entry to terminator, `analyze` walks the nodes twice:

- **Request-side first reader** — first node where any of: `Middleware(L7Request).needs_body()` is true; a `Check` reads `http.body`; a Fetch with retry enabled has `buffering: "force"`.
- **Response-side first reader** — first node after Fetch where `Middleware(L7Response).needs_body()` is true.

`lower` sets `collect_body_before = Some(BodySide::X)` on exactly the first reader. Downstream nodes on the same side do not re-set it: once `Body::Static`, the body stays static.

Runtime: at the flagged node, the executor calls `body.collect().await`, copying streaming frames into a contiguous `Bytes`, replacing `Body::Stream(...)` with `Body::Static(Bytes)`. `Body::Static` and `Body::Empty` are no-ops. Post-collect, the body is replay-safe; retry loops clone the `Bytes`.

`max_body_size` (per-rule, default 8 MiB) is enforced during collect. Request body exceeding produces `413`; response body exceeding produces `502`.

The runtime never asks "should I buffer this?". It follows a flag set at compile.

## Hash-consing

Dedup during `lower`:

- **`PredicateInst`** — keyed by full value (`Hash + Eq`). All predicates are pure functions; cross-phase sharing is sound (two rules checking `tls.sni == "x"`, one L4Peeked and one L7Request, share one `PredicateId`).
- **`MiddlewareInst`** — driven by statefulness, enforced at construction:

  | Kind                    | Dedup    | Key                                             |
  | ----------------------- | -------- | ----------------------------------------------- |
  | Internal stateless      | dedup    | `(name, canonical_args_json)`                   |
  | Internal stateful       | per-site | — (always distinct `MiddlewareId`)              |
  | External WASM stateless | dedup    | `(module_id, export_name, canonical_args_json)` |
  | External WASM stateful  | per-site | — (each site gets its own pool)                 |

Stateful dedup would silently merge two rules' state (e.g. two `rate_limit(rate=100)` collapsing to one bucket halves the effective rate). Not allowed.

`FetchInst` is not hash-consed — each Fetch node gets its own instance even when two rules proxy to the same upstream. Preserves per-rule metrics, retry config, and Fetch identity for flow-log attribution.

Hash-consing is an IR / memory optimization. It does not cache `test()` results or short-circuit middleware `run()`. Per-connection memoization of pure predicate reads is post-MVP work.

## Two error channels

Middleware returns `Result<Decision, Error>`:

- **`Ok(Decision::Short(_))`** — application-level refusal. The middleware did its job correctly. `Short::Response(r)` writes `r` to the client; `Short::Close(reason)` closes.
- **`Err(Error)`** — the middleware failed to execute. Plugin trapped, parse step hit unreachable, pool exhausted. Internal anomaly, not a designed-in outcome. Routes via `Node::Middleware.on_error` (see § _Compiled form_).

Collapsing both into one channel forces awkward config: every failure goes to the same fallback or the rule distinguishes via opaque error codes. Splitting separates "what do you want when the logic says no" (deterministic, `Decision::Short`) from "what do you want when the middleware itself is sick" (rare, `on_error`, a safety net).

The executor logs `Err(_)` to the flow log with the middleware's name and `Error::kind()` regardless of `on_error` choice.

## State migration on reload

Intentionally none. When `ArcSwap` installs a new graph, the old `Arc<FlowGraph>` drops and its `MiddlewareInst`s drop with it; the new graph's stateful middleware are constructed fresh.

| Middleware kind         | Reload effect                                                 |
| ----------------------- | ------------------------------------------------------------- |
| Internal stateless      | None — no state to preserve                                   |
| Internal stateful       | State resets — buckets refill, counters return to zero        |
| External stateless WASM | Pool drains naturally; new pool starts fresh                  |
| External stateful WASM  | Linear memory resets — new pool pre-allocates fresh instances |

State-preserving alternatives (identity-based migration, externalised KV) are out of scope. Vane is a small, fast, fully in-memory proxy; its HMR contract is "in-flight connections see no disruption", not "stateful middleware survive arbitrary config edits". State that must survive reloads belongs in a dedicated layer between vane and the origin, or inside the application itself. For DDoS-class coarse protection that must survive, see the L1 floor (daemon-scoped, `ArcSwap` does not touch it) — [`crates/core.md` § _Rate limit_](crates/core.md#rate-limit).

## Flow log verbosity

The walker emits two streams into `ctx.log: &Arc<dyn FlowLogSink>`:

| Mode                      | Content                                                                                                                                                                                                               |
| ------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `Trajectory` (default)    | Per request: one `FlowLogKind::Trajectory` event whose `data` is a serialised `FlowTrajectory` (entry + steps + outcome + timings). Plus per-connection milestones: `Terminate`, `Error`, `Upgrade`, `SecurityLimit`. |
| `Debug` (mgmt-API toggle) | Trajectory plus one event per walker step (`Check` / `Middleware` / `Fetch` / `Upgrade`). Used for incident drilling.                                                                                                 |

`tracing::trace!` per-step is independent; gated only by `RUST_LOG`.

Verbosity is read once when the listener constructs `FlowCtx`. In-flight connections retain the value they were built with; the toggle only affects connections accepted after the flip.

`FlowTrajectory` shape: `crates/core/src/flow_log.rs`. Granularity is node-level — predicate IDs and middleware args are not on the trajectory; operators trace by node id and look up `graph[node]` against the symbolic graph for detail.

Default sink composition (`crates/engine/src/flow_log_sink/`):

1. `RingBufferSink` — 10000-entry / 60-second sliding window, always present. Backs `tail_flow`.
2. `FileSink` — opt-in via `VANE_FLOW_LOG_FILE`. Append-only NDJSON. Writes go through a tokio mpsc into a background task so `emit` never blocks the executor on disk I/O.

## What the graph is not

- Not a tree of boxed trait objects. Nodes are statically typed; the executor is a small `match` over a fixed enum.
- Not a state machine in the usual sense. Walking is stateless other than `ConnContext`.
- Not user-writable directly. Operators edit rules; the compiler produces the graph. `vane compile <DIR>` exposes the compiled form for inspection.
