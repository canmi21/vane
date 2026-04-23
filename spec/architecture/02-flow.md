# Flow Model

The Flow model is Vane's central abstraction. Everything else in this directory exists to serve it.

## The funnel

Vane is a **single-trunk funnel**. Every byte entering `vaned` — whether a raw TCP stream, a UDP datagram, or an HTTP request decoded from H2/H3 — enters one dispatch engine. That engine decides what happens next by walking a single, compiled, immutable `FlowGraph`.

This is deliberate. Per-protocol pipelines (what v1 and v2 tried) fail because the axis "what protocol is this" and the axis "what does the user want to do with it" are independent. Encoding them as parallel code paths forces crosscut logic (e.g., "HTTPS and SSH both on :443") into special-case glue. The funnel inverts this: the dispatch engine is one thing, and decisions live in the graph.

## Rules, not FlowGraphs

Users do not write FlowGraphs. Users write **rules**:

```json
{
  "rule": "<unique-name>",
  "listen": ["<port-or-address>", ...],
  "match": [ <predicate>, ... ],
  "terminate": { <terminator-config> }
}
```

- `rule`: globally unique identifier (used for conflict resolution, log attribution, metrics).
- `listen`: ports or addresses this rule applies to.
- `match`: zero or more predicates. All must hold for the rule to match. Zero predicates = fallthrough / always match.
- `terminate`: what to do when the rule matches. See [`05-terminator.md`](05-terminator.md).

A predicate reads fields from the connection context (`transport`, `remote`, `tls.sni`, `http.header.host`, `http.body.contains(...)`, etc.). It **does not name hooks**. The compiler derives required hooks from predicate field access.

## Merge

Configuration is multi-file. Each file contains zero or more rules plus optional global settings. Merge is deterministic:

1. Enumerate files under the config directory, sorted lexicographically by path.
2. For each file, read `"order": N` from the top level (default 0). Stable-sort by `(order asc, filename lex)`.
3. Accumulate rules into a single `RuleSet`.
4. Duplicate `rule` names are an **error at merge time**. The user renames to override.
5. Global settings (listener bindings, management config, WASM pool sizes) follow last-write-wins with a merge log.

Output: a single canonical `MergedConfig` document, dumpable via `vane compile --dry-run`.

## Compile

Compilation is a pipeline of pure functions:

```
RuleSet
  ↓ merge       (dedup rule names, resolve order, emit conflict log)
MergedConfig
  ↓ analyze    (inspection level, specificity, LazyBuffer need per rule)
AnalyzedRuleSet
  ↓ lower      (group by listener, sort, build tree, flatten to Vec)
FlowGraph
  ↓ validate   (every leaf terminates; every reference resolves)
Arc<FlowGraph>
```

Each stage's input type fully determines its output. Stages are independently testable.

### analyze

For each rule, compute:

- **Inspection level** — the deepest field any predicate accesses (`L4 < L7-header < L7-body`).
- **Specificity** — number of predicates (tiebreaker among same-level rules).
- **LazyBuffer need** — whether any predicate reads `http.body.*` (propagates to the compiled path as a buffer-vs-stream flag).

### lower

1. Group analyzed rules by target listener port.
2. Within each group, sort by `(inspection level desc, specificity desc, name asc)`.
3. Build a decision tree: for each rule in order, emit `Check` nodes for its predicates and a `Terminate` leaf. Subsequent rules extend the `on_miss` branches.
4. Flatten the tree into `Vec<Node>`, assign `NodeId`s, rewrite edges to indices.
5. Hash-cons predicates — two rules with the same `tls.sni == "x"` share a `PredicateId`.

Post-MVP optimizations (subtree sharing, dead-node elimination) are additional passes over the flattened IR.

### validate

- Every `Node::Check`'s `on_match` and `on_miss` resolve to valid `NodeId`s.
- Every `Node::Middleware`'s `id` and `next` resolve.
- Every `Node::Fetch`'s `id` resolves. `next_response` and `next_tunnel` are each `Some(valid NodeId)` or `None` consistent with the Fetch variant's output modes (`HttpProxy` / `HttpSynthesize` → `next_response` required, `next_tunnel` forbidden; `L4Forward` → `next_tunnel` required, `next_response` forbidden; `WebSocketUpgrade` → both required). Referenced upstream addresses, WASM modules, and CGI binary paths exist and type-check.
- Every `Node::Terminate`'s referenced Terminator exists.
- The graph is acyclic.
- **Phase consistency** — every walk from an entry to a Terminator respects the phase state machine. `L4PeekMiddleware` / `L4BytesMiddleware` appear only on pre-Fetch L4 paths; `L7RequestMiddleware` only between L4→L7 upgrade and Fetch; `L7ResponseMiddleware` only between Fetch and `Terminator::WriteHttpResponse`; `Terminator::ByteTunnel` follows only `Fetch::L4Forward` or `Fetch::WebSocketUpgrade`. Violations are compile errors with the offending node and source rule named.

On any validation failure, compilation aborts with the offending node ID and source rule name. No partial `FlowGraph` is exposed.

A `FlowGraph` is never mutated after compilation. Reload re-compiles from scratch and `ArcSwap`s in the new `Arc<FlowGraph>`.

## The compiled form

The `FlowGraph` is a **flat, index-based intermediate representation**. Nodes, middleware instances, terminator instances, and predicate instances live in parallel `Vec`s; references between them are typed newtype indices.

```rust
pub struct FlowGraph {
    nodes:       Vec<Node>,
    predicates:  Vec<PredicateInst>,
    middlewares: Vec<MiddlewareInst>,
    fetches:     Vec<FetchInst>,
    terminators: Vec<Terminator>,
    entries:     HashMap<SocketAddr, NodeId>,  // per-listener entry points
    meta:        FlowGraphMeta,
}

pub enum Node {
    Check      { predicate: PredicateId, on_match: NodeId, on_miss: NodeId },
    Middleware { id: MiddlewareId, next: NodeId },
    Fetch {
        id:            FetchId,
        next_response: Option<NodeId>,   // followed when Fetch produces a Response
        next_tunnel:   Option<NodeId>,   // followed when Fetch produces a Tunnel
    },
    Terminate(TerminatorId),
}

pub struct NodeId(u32);
pub struct PredicateId(u32);
pub struct MiddlewareId(u32);
pub struct FetchId(u32);
pub struct TerminatorId(u32);
```

Rationale:

- **Cache locality** — `Node`s are contiguous; the CPU prefetcher loads adjacent nodes as a matter of course.
- **Subtree sharing** — two rules compiling to the same subgraph share nodes via shared `NodeId`s. Trivial with indices; hard with `Box<>`.
- **Compact memory** — `NodeId(u32)` is half the size of a pointer on 64-bit.
- **Stable serialization** — the flat form dumps as `{ nodes: [...], entries: { ... } }`. `vane compile --dry-run` output is diff-friendly JSON that survives `jq`.
- **Single allocation** — `Vec<Node>::with_capacity` then grow. Not N `Box::new` calls.

`impl Index<NodeId> for FlowGraph`, `Index<PredicateId>`, etc. give `graph[id]` ergonomics. The newtype wrappers prevent confusing a `NodeId` with a `PredicateId` at compile time.

## Execution model

The executor is an **iterative walker**. A single `async fn` holds a loop; the loop walks the flat graph by updating a `NodeId` cursor.

```rust
pub async fn execute(
    graph: &FlowGraph,
    entry: NodeId,
    ctx:   &mut Ctx<'_>,
) -> Result<(), Error> {
    let mut cur = entry;
    loop {
        match &graph[cur] {
            Node::Check { predicate, on_match, on_miss } => {
                let matched = graph[*predicate].test(ctx);
                cur = if matched { *on_match } else { *on_miss };
            }
            Node::Middleware { id, next } => {
                graph[*id].run(ctx).await?;
                cur = *next;
            }
            Node::Fetch { id, next_response, next_tunnel } => {
                // Fetch may produce either a Response (for HttpProxy / HttpSynthesize,
                // or WebSocketUpgrade when upstream rejects the upgrade) or a Tunnel
                // (for L4Forward, or WebSocketUpgrade on 101). Dispatch on the output
                // variant. The compiler guarantees the relevant `next_*` is `Some` for
                // each Fetch variant's reachable outputs (see validate section above).
                match graph[*id].fetch(ctx).await? {
                    FetchOutput::Response(_) => {
                        cur = next_response.expect("compile-time check ensures this is Some");
                    }
                    FetchOutput::Tunnel(_) => {
                        cur = next_tunnel.expect("compile-time check ensures this is Some");
                    }
                }
            }
            Node::Terminate(t) => {
                return graph[*t].run(ctx).await;
            }
        }
    }
}
```

Why iterative, not recursive:

- The entire execution is a single `Future`, a single state machine, a single allocation per request. Recursive `async fn` requires `Box::pin` per call — at 10k QPS with 10 nodes per request, that's 100k saved allocations per second.
- No stack depth concerns — graphs can be deep after predicate merging.
- The cursor `cur` is the complete execution state (plus `ctx`). Future features — flow log replay, coroutine-like pause/resume, checkpointing — are additive.

The `tracing` integration emits one event per loop iteration: `trace!(node_id = ?cur, kind = "check" | "mid" | "terminate")`. This is the flow log that shows "connection X visited node 42, matched predicate, moved to node 43."

## Pay-as-you-go as a compilation property

The inspection-level analysis enforces pay-as-you-go:

- Port whose rules are all L4-only → compiled subgraph has no L7 nodes. Connections never pay L7 cost.
- Port with mixed L4 + L7 rules → subgraph has a protocol-detection node. Only L7-matching traffic escalates.
- Port with all L7 rules → protocol detection is implicit (listener declared as `http`); L7 is always active.

Pay-as-you-go is not a runtime optimization. It is a compilation guarantee.

## Graph validity

A `FlowGraph` is valid iff:

1. Every leaf is a Terminator node.
2. Every internal node has at least one child; predicate branches (`on_match` and `on_miss`) both lead to valid subgraphs.
3. Every Terminator's referenced resource exists (upstream reachable at compile time is **not** required; upstream liveness is a runtime concern).
4. Every declared WASM module loads and its referenced exports exist.

Validity is checked at compile. A broken graph does not reach `ArcSwap`; the previous graph continues to serve.

## LazyBuffer: load-time decision

Bodies stream by default. They are buffered only when a middleware or Terminator on this path actually reads body bytes.

The decision is made at compile time per-path:

- The compiler walks each path from the entry listener to each reachable Terminator.
- If **any** middleware or Terminator on the path declares `needs_body: true`, that path is marked buffered.
- On a buffered path, the runtime accumulates body bytes from the first byte before invoking the middleware that needs them.
- On a streaming path, `Bytes` chunks pass through to the Terminator with zero intermediate copies.

The runtime never asks "should I buffer this?" It follows a flag set at compile.

## Hot reload

1. File watcher detects change under `/etc/vaned/`.
2. `vaned` re-reads, re-merges, re-compiles.
3. On compile success: `Arc::new(new_graph)` → `arc_swap.store(new)`. In-flight connections keep their old `Arc`; new connections see the new graph. Old `Arc` drops when the last in-flight user releases it.
4. On compile failure: log the error, surface it to the management API, do not swap. Old graph continues serving.

Reload is never destructive. A broken configuration change cannot take down a running daemon.

## What the graph is not

- Not a tree of boxed trait objects. Nodes are statically typed; the graph's execution engine is a small set of match arms over a fixed node enum.
- Not a state machine per se. FlowGraph walking is stateless (other than `ConnContext`); there is no per-graph global state.
- Not user-writable directly. Users edit rules; the compiler produces the graph. `vane compile --dry-run` exposes the compiled form for inspection, not for hand-editing.
