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

A predicate reads fields from the connection context (`transport`, `remote`, `tls.sni`, `http.header.host`, `http.body`, etc.) and applies an operator to the read value. It **does not name hooks**. The compiler derives required hooks from predicate field access. See `18-predicate-schema.md` for the grammar.

## Merge

Configuration is multi-file. Each file contains zero or more rules plus optional global settings. Merge is deterministic:

1. Enumerate files under the config directory, sorted lexicographically by path.
2. For each file, read `"order": N` from the top level (default 0). Stable-sort by `(order asc, filename lex)`.
3. Accumulate rules into a single `RuleSet`.
4. Duplicate `rule` names are an **error at merge time**. The user renames to override.
5. Global settings (listener bindings, management config, WASM pool sizes) follow last-write-wins with a merge log.

Output: a single canonical `MergedConfig` document, dumpable via `vane compile --dry-run`.

## Compile and link — two stages, two crates

Config becomes an executable graph in **two distinct phases**:

```
[vane-core]  (no hyper / rustls / wasmtime / tokio)
RuleSet
  ↓ merge       (dedup rule names, resolve order, emit conflict log)
MergedConfig
  ↓ expand      (preset expansion → RawRules)
RawRuleSet
  ↓ analyze     (inspection level, specificity, LazyBuffer tracks per rule; reads metadata provider)
AnalyzedRuleSet
  ↓ lower       (group by listener, sort, build tree, flatten to Vec, hash-cons predicates + stateless middleware)
SymbolicFlowGraph
  ↓ validate    (IR integrity: NodeId resolution, DAG, phase machine, predicate-field legality)
Arc<SymbolicFlowGraph>

[vane-engine]  (runtime; holds hyper / rustls / wasmtime / tokio)
Arc<SymbolicFlowGraph>
  ↓ link        (resolve SymbolicMiddlewareRef/SymbolicFetchRef names via factory registries;
                 feature-availability rejection; instantiate trait objects)
Arc<FlowGraph>  ← ArcSwap target; the executor reads this
```

The split resolves the "who owns `Arc<dyn _>`" question: `SymbolicFlowGraph` is pure IR (no trait objects, no Tokio, cheap to build, trivially JSON-serializable). `FlowGraph` is the linked form that embeds `Vec<MiddlewareInst>` and `Vec<FetchInst>` of `Arc<dyn Trait>`; it only exists in engine.

- `vane lint` and `vane compile --dry-run` link only `vane-core`; they produce `SymbolicFlowGraph` and serialize it to JSON. No hyper, no wasmtime, no tokio needed.
- `vaned` boot and reload run both stages: core `compile` then engine `link`. Both Arcs are cheap to swap; only the linked `FlowGraph` is `ArcSwap`-managed at runtime.

Each stage's input type fully determines its output. Stages are independently testable.

### analyze

For each rule, compute:

- **Inspection level** — the deepest field any predicate accesses (`L4-only < L4-peek < L7-header < L7-body`). See `18-predicate-schema.md` § _Field path → inspection level_ for the authoritative table.
- **Specificity** — number of predicates (tiebreaker among same-level rules).
- **LazyBuffer need** — whether any predicate reads `http.body.*` (propagates to the compiled path as a buffer-vs-stream flag).

### lower

1. Group analyzed rules by target listener port.
2. Within each group, sort by `(inspection level desc, specificity desc, name asc)`.
3. Build a decision tree: for each rule in order, emit `Check` nodes for its predicates and a `Terminate` leaf. Subsequent rules extend the `on_miss` branches.
4. Flatten the tree into `Vec<Node>`, assign `NodeId`s, rewrite edges to indices.
5. Hash-cons predicates and stateless middleware — two rules with the same `tls.sni == "x"` share a `PredicateId`; two rules using the same stateless middleware (e.g., `path_prefix "/api"`) share a `MiddlewareId`. Stateful middleware (e.g., `rate_limit`) is **never** shared across call sites — every rule using `rate_limit` gets its own `MiddlewareId` and its own bucket. See the "Hash-consing" section below.

Post-MVP optimizations (subtree sharing, dead-node elimination) are additional passes over the flattened IR.

### validate (core, IR-level)

The core validator runs on the freshly lowered `SymbolicFlowGraph` and checks structural correctness that does not depend on which features the final binary linked:

- Every `Node::Check`'s `on_match` and `on_miss` resolve to valid `NodeId`s.
- Every `Node::Middleware`'s `id` and `next` resolve; `on_error`, if `Some(_)`, resolves to a valid `NodeId` and that target's accepted-phase must include `cur_phase`'s out-phase equivalent (an `on_error` from an L7Request middleware cannot jump into an L4 subgraph).
- Every `Node::Upgrade`'s `next` resolves and must accept phase `L7Request`. `Upgrade` may only appear on a path between L4-phase execution and an L7-phase sub-graph; the DFS phase walker catches misplacement automatically.
- Every `Node::Fetch`'s `id` resolves. `next_response` and `next_tunnel` are each `Some(valid NodeId)` or `None` consistent with the Fetch kind's output modes (`HttpProxy` / `HttpSynthesize` → `next_response` required, `next_tunnel` forbidden; `L4Forward` → `next_tunnel` required, `next_response` forbidden; `WebSocketUpgrade` → both required).
- Every `Node::Terminate`'s referenced Terminator exists.
- The graph is acyclic.
- **Phase consistency** — every walk from an entry to a Terminator respects the phase state machine. The authoritative rules (accepted in-phases, out-phase) live in the [Phase state machine](#phase-state-machine) section below; the validator is a DFS from each `entries[addr]` starting in `Phase::L4Raw` that looks up each node in the transition table. Violations name the offending node, the source rule, and the expected vs. actual phase.

On any core validation failure, compilation aborts; no partial `SymbolicFlowGraph` is exposed.

### link (engine, feature + impl availability)

The engine's `FlowGraph::link` takes an `Arc<SymbolicFlowGraph>` and resolves each `SymbolicMiddlewareRef` / `SymbolicFetchRef` by name against the factory registries. This is where the build's actual compiled features decide what runs:

- **Feature availability** — every referenced middleware / Fetch kind must have a factory registered in this binary. A rule using `http/3` on a binary built `--no-default-features` (no `h3` feature) fails with `"this binary was built without the 'h3' feature — rebuild with --features h3 or remove the rule"`. Symbolic compile succeeds on any config; link rejects. See `16-crate-layout.md` § _Feature-off → rule compile-time rejection_.
- **Referenced resource presence** — upstream addresses parse, WASM module names map to loaded components, CGI binary paths exist.
- **Factory args acceptance** — each middleware / Fetch factory validates its args at construction; an arg shape the registered impl rejects fails link.

On any link failure, the symbolic graph is discarded; no `FlowGraph` is exposed, and the previously linked graph continues to serve.

Both stages are fail-closed: reload never swaps in a partially valid graph. A `FlowGraph` is never mutated after linking. Reload re-runs both stages from scratch and `ArcSwap`s in the new `Arc<FlowGraph>`.

## The compiled form

Two graph types, one crate each (see `16-crate-layout.md`):

- `SymbolicFlowGraph` (vane-core) — pure IR, `serde::Serialize`-able, no trait objects. Output of core's compile pipeline; input to engine's `link`. `vane compile --dry-run` outputs this.
- `FlowGraph` (vane-engine) — linked, executable, holds `Vec<MiddlewareInst>` and `Vec<FetchInst>` of trait objects. Output of `link`. Stored in the daemon's `ArcSwap<FlowGraph>`; read by the executor.

Both share the **flat, index-based** shape. Nodes, predicate instances, and terminator slots live in parallel `Vec`s; references between them are typed newtype indices.

```rust
// vane-core
pub struct SymbolicFlowGraph {
    nodes:       Vec<Node>,
    predicates:  Vec<PredicateInst>,
    middlewares: Vec<SymbolicMiddlewareRef>,    // name + args + metadata; no impl
    fetches:     Vec<SymbolicFetchRef>,         // name + args; no impl
    terminators: Vec<Terminator>,               // unit enum, fully symbolic
    entries:     HashMap<SocketAddr, NodeId>,   // per-listener entry points
    meta:        FlowGraphMeta,
}

// vane-engine
pub struct FlowGraph {
    symbolic:    Arc<SymbolicFlowGraph>,        // retained for dry-run / metrics attribution / flow log
    middlewares: Vec<MiddlewareInst>,           // Arc<dyn L4PeekMiddleware> etc. (constructed in link)
    fetches:     Vec<FetchInst>,                // Arc<dyn L4Fetch> / Arc<dyn L7Fetch> (constructed in link)
}

pub enum Node {
    Check {
        predicate: PredicateId,
        on_match:  NodeId,
        on_miss:   NodeId,
        collect_body_before: Option<BodySide>,
    },
    Middleware {
        id:       MiddlewareId,
        next:     NodeId,
        on_error: Option<NodeId>,                // Some → on Err(_) jump here; None → default fallback (see below)
        collect_body_before: Option<BodySide>,
    },
    Fetch {
        id:            FetchId,
        next_response: Option<NodeId>,           // followed when L7Fetch produces a Response
        next_tunnel:   Option<NodeId>,           // followed when L4Fetch runs, or L7Fetch::WS returns Tunnel
        collect_body_before: Option<BodySide>,
    },
    Upgrade {
        next: NodeId,                            // L4Peeked → L7Request phase transition
    },
    Terminate(TerminatorId),                     // terminators never trigger collects; bodies pass through
}

pub enum BodySide { Request, Response }

pub struct NodeId(u32);
pub struct PredicateId(u32);
pub struct MiddlewareId(u32);
pub struct FetchId(u32);
pub struct TerminatorId(u32);
```

`collect_body_before` is the compile-time LazyBuffer trigger. When set to `Some(BodySide::Request | Response)`, the executor performs `Body::collect().await` on the relevant side _before_ executing the node, replacing `Body::Stream(...)` with `Body::Static(Bytes)`. The analyze pass sets it on exactly the **first** node on each path that needs the buffered bytes (see the LazyBuffer section below); downstream nodes on the same side inherit the buffered state naturally because `Body::Static` does not revert.

`Node::Upgrade { next }` is the explicit L4→L7 phase boundary. It carries no parameters of its own — the concrete upgrade behavior (TLS termination? ALPN selection? which HTTP version?) is driven by the **listener config** that produced the `Arc<ConnContext>` this execution operates on. A rule whose `listen: [":80", ":443"]` produces two listeners; both share the same graph and the same `Upgrade` node, but `:80`'s upgrade skips TLS while `:443`'s performs TLS handshake + ALPN dispatch. Graph only says "upgrade here"; listener config says "with what".

`Node::Middleware.on_error` routes `Err(Error)` returns from a middleware. Default (`None`) is the **fail-safe tombstone**: L7 path writes a `500 Internal Server Error`; L4 path closes the connection with RST. A `Some(target)` jumps to `target` and the request continues there. This is the IR-level realization of the config-level `on_error` DSL (`"close"` / inline synth response) that `lower` resolves into a concrete `NodeId`. See `04-middleware.md` for the user-facing config shape and for why `Decision::Short` (application-level refusal) and `Err(_)` (internal anomaly) are two distinct channels.

Rationale:

- **Cache locality** — `Node`s are contiguous; the CPU prefetcher loads adjacent nodes as a matter of course.
- **Subtree sharing** — two rules compiling to the same subgraph share nodes via shared `NodeId`s. Trivial with indices; hard with `Box<>`.
- **Compact memory** — `NodeId(u32)` is half the size of a pointer on 64-bit.
- **Stable serialization** — `SymbolicFlowGraph`'s flat form dumps as `{ nodes: [...], entries: { ... }, middlewares: [...], ... }`. `vane compile --dry-run` serializes this directly — diff-friendly JSON that survives `jq`. `FlowGraph` is not serializable (trait objects); dry-run never needs it.
- **Single allocation** — `Vec<Node>::with_capacity` then grow. Not N `Box::new` calls.

`impl Index<NodeId> for SymbolicFlowGraph` / `Index<PredicateId>` etc. give `graph[id]` ergonomics; engine's `FlowGraph` exposes the same idiom plus `Index<MiddlewareId> -> &MiddlewareInst` and `Index<FetchId> -> &FetchInst` for the executor. The newtype wrappers prevent confusing a `NodeId` with a `PredicateId` at compile time.

### Compiled predicate instances

`PredicateInst` is the compile-time-validated runtime form of a rule's `match` predicate. Config-time (JSON) shape is defined in `18-predicate-schema.md`; the transformation from config to compiled is part of the `lower` pass.

```rust
pub struct PredicateInst {
    pub path: FieldPath,
    pub op:   CompiledOperator,
}

pub enum CompiledOperator {
    Equals(CompiledValue),
    NotEquals(CompiledValue),
    Contains(bytes::Bytes),
    NotContains(bytes::Bytes),
    Prefix(bytes::Bytes),
    Suffix(bytes::Bytes),
    Matches(fancy_regex::Regex),          // already compiled; size + backtrack limits applied at construction
    In(Vec<CompiledValue>),
    NotIn(Vec<CompiledValue>),
    Gt(i64), Gte(i64), Lt(i64), Lte(i64),
    Cidr(ipnet::IpNet),
}

pub enum CompiledValue {
    Str(std::sync::Arc<str>),
    Bytes(bytes::Bytes),
    Int(i64),
    Bool(bool),
    Addr(std::net::IpAddr),
}

impl PredicateInst {
    pub fn test(&self, view: &PredicateView<'_>) -> bool { /* dispatch on path + op; see 18-predicate-schema.md */ }
}
```

`PredicateInst` implements `Hash + Eq` so the `lower` pass can hash-cons equivalent predicates — two rules both checking `tls.sni == "example.com"` share one `PredicateId`. `fancy_regex::Regex` compares by pattern source string; `ipnet::IpNet` compares by canonical form. MiddlewareInst dedup follows a separate policy driven by statefulness (see "Hash-consing" section).

### FlowGraph metadata

```rust
pub struct FlowGraphMeta {
    pub version_hash: [u8; 32],          // SHA-256 over the canonical MergedConfig JSON
    pub compiled_at:  std::time::SystemTime,
    pub source_files: Vec<std::path::PathBuf>,  // files that contributed to this graph
    pub feature_set:  &'static [&'static str],  // snapshot of daemon-enabled Cargo features

    // Routing helpers populated by the lower pass; queried by the executor.
    // Both maps key on the original entry NodeId passed to execute(), so a
    // graph with multiple listener entries can route each entry's fallback
    // / short-circuit independently.
    pub short_circuit_response_entry: std::collections::BTreeMap<NodeId, NodeId>,
    pub default_fallback_l4:          std::collections::BTreeMap<NodeId, NodeId>,
    pub default_fallback_l7:          std::collections::BTreeMap<NodeId, NodeId>,
}

impl FlowGraphMeta {
    /// Where to jump when an `L7RequestMiddleware` returns
    /// `Decision::Short(Response)`. The lower pass populates one entry per
    /// L7 entry; an L4-only entry has no response phase so the lookup is
    /// never reached at runtime (validator confirms).
    pub fn short_circuit_response_entry(&self, entry: NodeId) -> NodeId {
        self.short_circuit_response_entry.get(&entry).copied()
            .expect("lower invariant: every L7 entry has a response-side entry")
    }

    /// Where to jump when a middleware returns `Err(_)` and its IR
    /// `on_error` is `None`. Phase-aware: L4 phases land at a synthesised
    /// `Terminate(Close)`; L7 phases land at a synthesised
    /// `Terminate(WriteHttpResponse)` carrying a 500 body. Lower
    /// synthesises one fallback node per (entry, phase-side) and records
    /// the NodeId here; nodes are hash-consed across entries that share
    /// a fallback shape.
    pub fn default_fallback(&self, entry: NodeId, phase: Phase) -> NodeId {
        match phase {
            Phase::L4Raw | Phase::L4Peeked | Phase::Tunnel => {
                self.default_fallback_l4.get(&entry).copied()
                    .expect("lower invariant: every entry has an L4 fallback")
            }
            Phase::L7Request | Phase::L7Response => {
                self.default_fallback_l7.get(&entry).copied()
                    .expect("lower invariant: every L7 entry has an L7 fallback")
            }
        }
    }
}
```

`version_hash` is returned by the management API's `get_active_config` verb and gates reload idempotency — `ArcSwap::store` runs only when the new graph's hash differs from the currently active one.

The two routing helpers (`short_circuit_response_entry`, `default_fallback`) are the bridge between the lower pass's synth nodes and the executor's hot path. Lower-pass synthesis is keyed on the `entry: NodeId` that `FlowGraph::entries` maps a listener `SocketAddr` to; the executor receives the same `entry` value as a function parameter and uses it as the lookup key. Future synthesis modes (e.g. user-supplied `on_error: { response: ... }` blocks generating per-call-site fallbacks) are additive — they extend lower without changing the helper shape.

These helpers are stubbed in the S1-15 executor (the `Short(Response)` arm and the `on_error == None` arm both return `Error::internal(..)` placeholders); the wiring lands when the lower pass synthesises the fallback subgraphs in a later S1 chunk.

## Phase state machine

Every position in a compiled graph belongs to exactly one **phase**. Phases define which middleware kinds and which Fetch / Terminator variants are legal at a given point. The state machine is the authoritative contract that the `validate` pass enforces and that the four middleware traits (see `04-middleware.md`) make compile-checkable.

```rust
pub enum Phase {
    L4Raw,       // pre-peek: TCP/UDP socket exists, no bytes read, no TLS
    L4Peeked,    // PeekResult.buffer populated; TLS ClientHello may be parsed
    L7Request,   // Request decoded from HTTP; entering request middleware chain
    L7Response,  // Response produced by Fetch; entering response middleware chain
    Tunnel,      // byte-bidirectional forwarding handed to Terminator::ByteTunnel
}
```

### Transition table

A single table drives both the validator and the runtime walker. Reading this table top-down: for a given `Node`, the "In-phase(s)" column lists phases the walker may be in when it reaches that node, and "Out-phase" is the phase it transitions into after executing the node.

| Node kind                                           | In-phase(s) accepted  | Out-phase                              |
| --------------------------------------------------- | --------------------- | -------------------------------------- |
| `Check`                                             | any                   | `= In` (phase is pass-through)         |
| `Middleware(L4Peek)`                                | `L4Raw` \| `L4Peeked` | `L4Peeked` (forces peek buffer)        |
| `Middleware(L4Bytes)`                               | `L4Raw` \| `L4Peeked` | `= In`                                 |
| `Upgrade`                                           | `L4Raw` \| `L4Peeked` | `L7Request`                            |
| `Middleware(L7Request)`                             | `L7Request`           | `L7Request`                            |
| `Middleware(L7Response)`                            | `L7Response`          | `L7Response`                           |
| `Fetch(FetchInst::L4, L4Forward)`                   | `L4Raw` \| `L4Peeked` | `Tunnel`                               |
| `Fetch(FetchInst::L7, HttpProxy \| HttpSynthesize)` | `L7Request`           | `L7Response`                           |
| `Fetch(FetchInst::L7, WebSocketUpgrade)`            | `L7Request`           | `L7Response` \| `Tunnel` (bi-outcome)  |
| `Terminate(WriteHttpResponse)`                      | `L7Response`          | (terminal — ends execution)            |
| `Terminate(ByteTunnel)`                             | `Tunnel`              | (terminal — ends execution)            |
| `Terminate(Close)`                                  | any                   | (terminal — drops connection silently) |

The `Upgrade` node is the explicit L4→L7 phase boundary inserted by the `lower` pass on every L7 path. Its in-phase is `L4Raw | L4Peeked`: **pure-L7 listeners** (e.g., an `Http` listener whose ALPN negotiation alone reveals the version — no SNI-routing, no mix-port protocol sniffing) transition directly `L4Raw → L7Request` via `Upgrade`; **mixed-posture listeners** (e.g., SNI-based tenant routing, port-443-for-both-HTTPS-and-SSH) run a `Middleware(L4Peek)` first, advancing `L4Raw → L4Peeked`, and then `Upgrade` fires `L4Peeked → L7Request`. The peek is optional but never backwards-incompatible.

The node itself carries no configuration — the protocol-stack initialization (TLS handshake, ALPN dispatch, HTTP version selection) is driven by the listener config attached to the `Arc<ConnContext>` at runtime. A listener without TLS termination runs `Upgrade` as "HTTP decode only"; a TLS-terminating listener runs it as "handshake + ALPN + HTTP decode". Graph specifies "upgrade here"; listener config specifies "with what posture".

`Terminate(Close)` is the default-miss terminator. When the `lower` pass synthesizes fallback paths (no rule matched), it emits `Terminate(Close)` — the executor silently closes the underlying transport (TCP RST / QUIC stream reset / Unix-socket shutdown) and emits a `FlowLogKind::Terminate` event with a `CloseReason::PolicyDenied("no matching rule")`. `Close` is phase-agnostic: unmatched paths can terminate in any phase. See `05-terminator.md` § _Variants_ for the full discussion of when `Close` fires vs the two content-bearing terminators.

Entries always start in phase `L4Raw`. `FlowGraph::entries` maps each listener's `SocketAddr` to a `NodeId` whose accepted in-phase must include `L4Raw`.

### Validator algorithm

```
for (addr, entry) in graph.entries {
    visit(entry, Phase::L4Raw)
}

fn visit(node_id, phase):
    if (node_id, phase) in seen: return        // cycle / join: already checked
    seen.insert((node_id, phase))

    let node = graph[node_id]
    if phase not in transition_table[node.kind()].in_phases:
        error(node_id, phase, "phase mismatch")

    let out = transition_table[node.kind()].out_phase(phase)
    for next in node.successors():
        visit(next, out)
```

DFS from each entry. The `seen` set is keyed by `(node_id, phase)` — a shared subgraph may be reachable in more than one phase context (rare, but legal for `Check` nodes that sit on a join point), so the key captures both. Cycles are caught by the preceding acyclicity check in `validate`.

### Error format

Phase violations point at the offending node plus the source rule:

```
error: phase mismatch at NodeId(42)
       expected one of: L7Request
       got: L4Peeked
       from rule `web-api` at rules/30-api.json:17
       cause: L7Request middleware placed before L4→L7 upgrade edge
```

The `source` field on `RawRule` (see `14-presets.md`) plus the `MiddlewareRef::name` on the violating node are the inputs to this message. Preset expansion preserves the original preset's source location, so a bad preset expansion points at the preset invocation, not at synthetic inner names.

### Why a table, not ad-hoc checks

A single transition table is the Rust-proxy analogue of an ISA encoding table or an LLVM IR verifier: adding a new `Node` kind or a new `Fetch` variant requires one table row, and the validator + any future IR dumpers + documentation stay in lockstep. Scattering the same rules across `lower`, `validate`, and executor match arms is where IR checkers grow silent gaps.

## Execution model

The executor is an **iterative walker**. A single `async fn` holds a loop; the loop walks the flat graph by updating a `NodeId` cursor and maintaining the per-phase owned state slots.

`execute` returns an `ExecutorOutput` describing what the caller — listener accept-loop task for L4 entries, hyper service-fn for L7 sub-graph dispatch — must do next:

```rust
pub enum ExecutorOutput {
    /// `Terminator::Close` walked, or any path the executor finalised
    /// without producing a response or tunnel. Caller does nothing
    /// further; transport drop-glue closes.
    Closed,
    /// `Terminator::WriteHttpResponse` walked. Caller serialises this
    /// `Response` onto the client socket. The hyper service-fn returns
    /// it from the H1 (and later H2 / H3) handler; the executor itself
    /// is socket-free in the L7 path.
    HttpResponse(Response),
    /// `Terminator::ByteTunnel` walked. Executor already drove
    /// `tokio::io::copy_bidirectional` (raced against `ctx.cancel`)
    /// to completion; the close reason — `Graceful` / `ProtocolError` /
    /// `Cancelled` — was sent through `Tunnel.close_reason_tx`. Caller
    /// does nothing further.
    Tunneled,
}
```

The `Closed` variant is also produced by the `Node::Upgrade` arm: once `drive_h1_server` finishes (client EOF or last `Connection: close` response written), the outer L4 `execute` propagates `Ok(ExecutorOutput::Closed)` back to the listener.

```rust
pub async fn execute(
    graph: &Arc<FlowGraph>,          // Arc so the Upgrade arm can clone it into a hyper service-fn closure
    entry: NodeId,
    input: ExecutorInput,            // L4Conn (L4 entries) or Request (L7 entries)
    conn:  &Arc<ConnContext>,
    ctx:   &mut FlowCtx,
) -> Result<ExecutorOutput, Error> {
    // Phase-scoped owned slots. The phase state machine guarantees at most one
    // is `Some` at any time; `.take().expect("phase invariant")` is sound.
    let mut l4:     Option<L4Conn>  = /* from input if L4 entry */;
    let mut req:    Option<Request> = /* from input if L7 entry */;
    let mut resp:   Option<Response> = None;
    let mut tunnel: Option<Tunnel>  = None;

    let mut cur = entry;
    loop {
        // Precondition: eager-buffer the relevant body if this node demands it.
        // The flag is set on the FIRST node (on each side) whose execution requires
        // a replayable Body::Static — see LazyBuffer section below.
        if let Some(side) = graph[cur].collect_body_before() {
            collect_body(side, req.as_mut(), resp.as_mut()).await?;
            // After this await, the targeted body is Body::Static(Bytes);
            // any downstream node on this side observes that without re-collecting.
        }

        match &graph[cur] {
            Node::Check { predicate, on_match, on_miss, .. } => {
                let view = PredicateView::build(conn, req.as_ref(), resp.as_ref(), l4.as_ref());
                let matched = graph[*predicate].test(&view);
                cur = if matched { *on_match } else { *on_miss };
            }

            Node::Middleware { id, next, on_error, .. } => {
                // Middleware error channel is binary:
                //   Ok(Decision::Continue)           → proceed to `next`
                //   Ok(Decision::Short(resp|close))  → application-level refusal
                //   Err(_)                           → internal anomaly → route via on_error or default fallback
                let outcome = match &graph[*id] {
                    MiddlewareInst::L4Peek(m) => {
                        let peek = conn.peek().expect("phase invariant").buffer.as_ref();
                        m.run(peek, conn, ctx).await
                    }
                    MiddlewareInst::L4Bytes(m) => {
                        let l4_ref = l4.as_mut().expect("phase invariant");
                        m.run(l4_ref, conn, ctx).await
                    }
                    MiddlewareInst::L7Request(m) => {
                        let req_ref = req.as_mut().expect("phase invariant");
                        m.run(req_ref, conn, ctx).await
                    }
                    MiddlewareInst::L7Response(m) => {
                        let resp_ref = resp.as_mut().expect("phase invariant");
                        m.run(resp_ref, conn, ctx).await
                    }
                    MiddlewareInst::Wasm(w) => w.invoke(conn, ctx, ...).await,
                };
                match outcome {
                    Ok(Decision::Continue)                 => cur = *next,
                    Ok(Decision::Short(Short::Response(r))) => {
                        drop(req.take());                         // Request dies on request-side short
                        resp = Some(r);
                        cur = graph.meta.short_circuit_response_entry(entry);
                    }
                    Ok(Decision::Short(Short::Close(reason))) => return Err(Error::closed(reason)),
                    Err(e) => {
                        ctx.log.emit_middleware_error(*id, &e);
                        cur = match on_error {
                            Some(target) => *target,
                            None         => graph.meta.default_fallback(entry, cur_phase()),
                            // default_fallback returns a synth Terminate
                            // node id keyed on (entry, phase): L4 phases →
                            // Terminate(Close); L7 phases → Terminate(
                            // WriteHttpResponse with 500). See the FlowGraph
                            // metadata helpers above.
                        };
                    }
                }
            },

            Node::Fetch { id, next_response, next_tunnel, .. } => match &graph[*id] {
                FetchInst::L7(f) => {
                    let r = req.take().expect("phase invariant");     // Request is consumed here
                    match f.fetch(r, conn, ctx).await? {
                        L7FetchOutput::Response(rp) => {
                            resp = Some(rp);
                            cur = next_response.expect("validator guarantees Some on L7 paths");
                        }
                        L7FetchOutput::Tunnel(t) => {
                            tunnel = Some(t);
                            cur = next_tunnel.expect("validator guarantees Some for WebSocketUpgrade");
                        }
                    }
                }
                FetchInst::L4(f) => {
                    let c = l4.take().expect("phase invariant");      // L4Conn is consumed here
                    let t = f.fetch(c, conn, ctx).await?;
                    tunnel = Some(t);
                    cur = next_tunnel.expect("validator guarantees Some on L4 paths");
                }
            },

            Node::Upgrade { next } => {
                // L4Raw / L4Peeked → L7Request. The L4 connection is handed
                // to a protocol-specific HTTP server driver. For each decoded
                // request, the driver constructs a fresh `FlowCtx` (sharing
                // the outer ctx's `log` / `cancel` / `verbosity` but with a
                // fresh `TrajectoryBuilder`) and calls
                // `execute(&graph, *next, ExecutorInput::L7(req), &conn,
                // &mut ctx)`. The L7 path's `ExecutorOutput::HttpResponse(r)`
                // flows back to the driver, which serialises `r` onto the
                // wire. When the underlying connection ends, the driver
                // returns `Ok(ExecutorOutput::Closed)` and the outer L4
                // `execute` returns it too.
                //
                // S1-17 ships H1 cleartext via `hyper::server::conn::http1`.
                // H2 sits next to H1 once TLS / ALPN are wired (S1-26); H3
                // follows the same shape via `h3::server::Connection` on a
                // `quinn::Endpoint`.
                let l4_conn = l4.take().expect("phase invariant");
                return drive_h1_server(
                    l4_conn, Arc::clone(graph), *next, Arc::clone(conn),
                    Arc::clone(&ctx.log), ctx.cancel.clone(), ctx.verbosity,
                ).await;
            }

            Node::Terminate(t) => return match &graph[*t] {
                Terminator::WriteHttpResponse => {
                    // Hand the Response back to the caller via the
                    // `ExecutorOutput::HttpResponse` variant. The caller —
                    // typically the hyper service-fn spawned at
                    // `Node::Upgrade` — returns the Response to its protocol
                    // stack which serialises it onto the wire. The executor
                    // itself is socket-free in the L7 path; this keeps it
                    // composable with hyper's request-response model and
                    // the future `h3::server` analogue.
                    let r = resp.take().expect("phase invariant");
                    Ok(ExecutorOutput::HttpResponse(r))
                }
                Terminator::ByteTunnel => {
                    // Driven inside the executor (`tokio::io::copy_bidirectional`
                    // raced against `ctx.cancel.cancelled()`). Caller does
                    // nothing further; the close reason is sent through
                    // `Tunnel.close_reason_tx` out-of-band.
                    drive_byte_tunnel(
                        tunnel.take().expect("phase invariant"), &ctx.cancel,
                    ).await;
                    Ok(ExecutorOutput::Tunneled)
                }
                Terminator::Close => {
                    // Phase-agnostic silent drop. Drop any owned slot; the
                    // accept-loop's drop-glue closes the underlying socket
                    // (TCP RST / QUIC stream reset / Unix shutdown). Emit
                    // a Terminate FlowLog event with PolicyDenied("no
                    // matching rule") so operators see the drop. No body
                    // ever travels through this terminator.
                    drop((l4.take(), req.take(), resp.take(), tunnel.take()));
                    Ok(ExecutorOutput::Closed)
                }
            },
        }
    }
}
```

Ownership summary — every owned resource has exactly one consumer (type system enforced, not a convention):

| Resource   | Created by                                                     | Consumed by                                                      |
| ---------- | -------------------------------------------------------------- | ---------------------------------------------------------------- |
| `L4Conn`   | accept loop                                                    | `L4Fetch::fetch` (moved into `Tunnel.upstream`) OR L4→L7 upgrade |
| `Request`  | L4→L7 upgrade (HTTP decoder)                                   | `L7Fetch::fetch` OR dropped on `Decision::Short(Response)`       |
| `Response` | `L7Fetch::fetch` OR a short-circuit from `L7RequestMiddleware` | `Terminator::WriteHttpResponse`                                  |
| `Tunnel`   | `L4Fetch::fetch` OR `L7Fetch::fetch` (WS-101)                  | `Terminator::ByteTunnel`                                         |

Why iterative, not recursive:

- The entire execution is a single `Future`, a single state machine, a single allocation per request. Recursive `async fn` requires `Box::pin` per call — at 10k QPS with 10 nodes per request, that's 100k saved allocations per second.
- No stack depth concerns — graphs can be deep after predicate merging.
- The cursor `cur` + owned slots `(l4, req, resp, tunnel)` are the complete execution state. Future features — flow log replay, coroutine-like pause/resume, checkpointing — are additive.

The `tracing` integration emits one event per loop iteration: `trace!(node_id = ?cur, kind = "check" | "mid" | "fetch" | "terminate")`. This is the flow log that shows "connection X visited node 42, matched predicate, moved to node 43."

## Flow log verbosity

The walker emits two structured streams into `ctx.log: &mut dyn FlowLogSink`:

| Mode (`FlowLogVerbosity`) | What lands in the sink                                                                                                                                                                                                                                                                         |
| ------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `Trajectory` (default)    | Per request: one `FlowLogKind::Trajectory` event whose `data` carries a serialised `FlowTrajectory` (entry + step list + outcome + timings). Plus the existing per-connection milestone events: `Terminate`, `Error`, `Upgrade`, `SecurityLimit`. The trajectory replaces the per-step stream. |
| `Debug` (mgmt-API toggle) | Everything `Trajectory` emits, plus one `FlowLogEvent` per walker step (`Check` / `Middleware` / `Fetch` / `Upgrade`). Used for incident-time inspection; not for production volumes.                                                                                                          |

`tracing::trace!` per-step is independent of this knob — it always fires, gated only by `RUST_LOG`. Verbosity gates only the structured `ctx.log` stream that flows out to management-API consumers.

The verbosity is read once when the listener constructs `FlowCtx` for a new connection, from a daemon-global `engine::VerbosityState` (`AtomicU8`). In-flight connections retain whatever verbosity they were built with; the toggle only affects connections accepted after the flip.

`FlowTrajectory` shape (defined in `vane_core::flow_log`):

```rust
pub struct FlowTrajectory {
    pub conn:           ConnId,
    pub entry:          NodeId,
    pub steps:          Vec<TrajectoryStep>,
    pub outcome:        TrajectoryOutcome,
    pub started_at_ms:  u64,
    pub finished_at_ms: u64,
}

pub struct TrajectoryStep {
    pub node:   NodeId,
    pub kind:   FlowLogKind,        // Check / Middleware / Fetch / Upgrade
    pub branch: Option<bool>,       // Check: Some(matched); other steps: None
}

pub enum TrajectoryOutcome {
    Terminated { node: NodeId, terminator: TerminatorOutcomeKind },
    Error      { node: NodeId, message: Cow<'static, str> },
}

pub enum TerminatorOutcomeKind { Close, WriteHttpResponse, ByteTunnel }
```

Granularity is **node-level** — `predicate_id` and middleware/fetch instance arguments are not on the trajectory. Operators trace by node id; if they need predicate detail, they look up `graph[node].predicate_id` against the symbolic graph.

### Default sink composition

The daemon's `FlowLogSink` is a `FanoutSink` of:

1. `RingBufferSink` (10_000-entry / 60-second sliding window). Always present. Backs the management API's `tail_flow_log` verb — both the live tail and the recent-window backfill.
2. `FileSink` — opt-in via `VANE_FLOW_LOG_FILE=<path>`. Append-only NDJSON. Writes go through a tokio mpsc channel into a background writer task so `emit` never blocks the executor on disk I/O.

`Trajectory` is a single-event-per-request summary that fits a one-line view; the management UI's default panel shows the latest N trajectories from the ring buffer. `Debug` mode supplements this for live incident drilling.

## Hash-consing

Dedup during `lower` so that two rules expressing the same logical shape share IR storage. Two tables, one per kind:

**`PredicateInst` dedup** (all predicates are pure functions; always safe):

- Key: the full `PredicateInst` value, compared by `Hash + Eq`.
- Cross-phase: one `PredicateId` shared across L4 and L7 Checks; the field path's read code is the same at every phase that admits the read (see `18-predicate-schema.md`).

**`MiddlewareInst` dedup** (driven by statefulness, enforced at construction):

| Kind                     | Dedup policy | Key                                             |
| ------------------------ | ------------ | ----------------------------------------------- |
| Internal stateless       | dedup        | `(name, canonical_args_json)`                   |
| Internal stateful        | per-site     | — (always distinct `MiddlewareId`)              |
| External WASM, stateless | dedup        | `(module_id, export_name, canonical_args_json)` |
| External WASM, stateful  | per-site     | — (each site gets its own instance pool)        |

Stateless dedup is safe because calling the same middleware with the same args from two call sites has no observable difference — there is no per-instance state. Stateful dedup would be **unsafe**: two rules both declaring `rate_limit(rate=100)` each want their own token bucket; collapsing them into one shared bucket silently halves the effective rate across the two rules. The `MiddlewareRegistry` (see `04-middleware.md`) knows each middleware's statefulness and routes construction accordingly.

Canonical args: serde's canonical JSON form (keys sorted, no whitespace, `null` fields omitted). Two rule files that write args in different key orders produce identical hashes.

**`FetchInst`** is not hash-consed in MVP. Each `Fetch` node gets its own `FetchInst` even when two rules proxy to the same `HttpUpstream` — this preserves per-rule metrics, per-rule retry config, and keeps Fetch identity stable for flow-log attribution. Revisit if profiling shows this matters.

**Runtime semantics**: hash-consing is an IR / memory optimization. It **does not** cache `test()` results or short-circuit middleware `run()`. Every `execute()` pass that walks through a Node executes the node's work; two HTTP/2 streams on one connection each walk their own L7 path and each execute every Check / Middleware they touch. Per-connection memoization of pure predicate reads (`tls.sni`, `remote.ip`) is a post-MVP optimization.

## Pay-as-you-go as a compilation property

The inspection-level analysis enforces pay-as-you-go:

- Port whose rules are all L4-only → compiled subgraph has no L7 nodes. Connections never pay L7 cost.
- Port with mixed L4 + L7 rules → subgraph has a protocol-detection node. Only L7-matching traffic escalates.
- Port with all L7 rules → protocol detection is implicit (listener declared as `http`); L7 is always active.

Pay-as-you-go is not a runtime optimization. It is a compilation guarantee.

## Graph validity

The authoritative validity rules are split across two passes, defined above:

- **Structural / IR validity** — `validate (core, IR-level)` section. Every leaf is a Terminator; every internal node has at least one child; predicate branches (`on_match` / `on_miss`) resolve; phase consistency holds; the graph is acyclic. These checks run on `SymbolicFlowGraph` in `vane-core` and never touch impls.
- **Feature / impl availability** — `link (engine, feature + impl availability)` section. Every referenced middleware / Fetch kind has a factory in this binary; referenced upstream addresses / WASM modules / CGI binary paths exist. These checks run on the factory registries in `vane-engine`.

A broken graph at either pass does not reach `ArcSwap`; the previous graph continues to serve.

## LazyBuffer: load-time decision, two independent tracks

Bodies stream by default. They are buffered only where a node on this path actually needs replayable bytes. **Request body and response body are analyzed as two independent tracks** — a single rule can be request-buffered and response-streaming, or vice versa, or both, or neither.

### Per-side analysis

For each path from an entry to a terminator, the analyze pass walks the nodes twice:

**Request-side first-reader**: first node (in execution order) where any of the following holds:

- the node is a `Middleware(L7Request)` whose impl declares `needs_body() == true`;
- the node is a `Check` whose predicate reads the `http.body` field path (always request-side in the current field grammar — see `18-predicate-schema.md`);
- the node is a `Fetch(L7Fetch)` whose FetchInst has retry enabled (retry requires a replayable request).

**Response-side first-reader**: first node (in execution order, after `Fetch`) where any of the following holds:

- the node is a `Middleware(L7Response)` whose impl declares `needs_body() == true`;
- (reserved) a `Check` whose predicate reads a response-side body field — no such field is defined today; placeholder for future extension.

For each side, the lower pass sets `collect_body_before = Some(BodySide::X)` on exactly the first-reader node. Nodes downstream on the same side do **not** re-set the flag: once `Body::Static`, the body stays static.

A path with no request-side first-reader stays request-streaming end-to-end; same for the response side. The two tracks are fully orthogonal.

### Where each track lives on the path

The two tracks occupy disjoint segments of the execution, separated by `L7Fetch`:

```
entry ─── ... ─── [request-side track] ─── L7Fetch ─── [response-side track] ─── Terminate
                  ^^^^^^^^^^^^^^^^^^^^     ^^^^^^^^    ^^^^^^^^^^^^^^^^^^^^^
                  request body lives       request      response body is
                  here; flag may fire      is           produced here; flag
                  on the first reader      consumed     may fire on the first
                  between entry and        here;        reader between Fetch
                  Fetch                    request-     and Terminate
                                           side track
                                           ends
```

- A **request-side `collect_body_before` flag is legal only on nodes between the entry and the L7Fetch** (i.e., nodes in phase `L7Request`). After `L7Fetch` the `Request` has been consumed; there is no body to collect, so the flag would be meaningless.
- A **response-side `collect_body_before` flag is legal only on nodes between the `L7Fetch.next_response` edge and the `Terminator::WriteHttpResponse`** (i.e., nodes in phase `L7Response`). Before `L7Fetch` there is no `Response` yet.
- The analyze pass runs its two first-reader searches over these two disjoint segments. The fact that a path is request-buffered has **no effect** on whether it is response-buffered — whether and where to collect the response body is re-analyzed from scratch starting at `L7Fetch.next_response`.

This is also why the validator does not need a "carry the buffer state across Fetch" rule: the two tracks never interact, and the `collect_body_before` flag space on each node is the enum `Option<BodySide>` whose `BodySide::Request` and `BodySide::Response` values live on different phase segments by construction.

### Runtime behavior

The executor's pre-node check (`02-flow.md` executor pseudocode) performs `body.collect().await` at the flagged node, **copying streaming frames into a single contiguous `Bytes`**:

- `Body::Stream(...)` → `Body::Static(Bytes)` — multi-frame aggregation, real memory copy (drives every protocol-specific ingress, since hyper Incoming / H3Body / plugin producers all live behind `Body::Stream`)
- `Body::Static(_)` / `Body::Empty` → no-op (already terminal)

This is not a type relabel — it is the explicit cost of LazyBuffer triggering. Triggering buffering means paying the full-body allocation + copy at this point; the stream story ends here for this side. The cost model is honest: no LazyBuffer = zero vane-layer copy end-to-end (modulo the inner-crate costs documented in `03-types.md`), LazyBuffer fires = one aggregate copy of the body on the triggered side.

Post-collect, the body is replay-safe: subsequent readers, including retry loops and response-side middleware that read after a mutation, get stable `&Bytes`. `max_body_size` (`03-types.md`) is enforced during collect; exceeding the limit produces `413` (request side) or `502` (response side).

The runtime never asks "should I buffer this?" It follows a flag set at compile.

### Worked example: request-streamed, response-buffered

Rule: `/api/upload` → H2 upstream, no body predicates, no request-phase middleware that inspects body. A response-phase middleware `response_filter` declares `needs_body() == true` (rewrites the response body).

- **analyze** sets `collect_body_before = None` on every node between entry and Fetch — request-side track is empty. It sets `collect_body_before = Some(BodySide::Response)` on the first node after `L7Fetch.next_response` that is `response_filter`.
- **runtime**: Request body streams from the client decoder to the upstream encoder unmodified — the H2 connection carries frames as they arrive. After `HttpProxyFetch` returns a `Response` whose `Body` is `Body::Stream(Box::pin(hyper_incoming))`, the executor reaches the response-side first-reader node, runs `resp.body_mut().collect().await` (copying all response frames into one `Bytes`), replaces with `Body::Static(bytes)`, then `response_filter` mutates it, then the egress encoder writes it with a computed `Content-Length` (or chunked framing if the egress is H1 and the body carries trailers — see `03-types.md`).

The reverse mix (request-buffered + response-streaming) is symmetric — the executor switches per side based on the flag, no global "this request is buffered" mode.

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
