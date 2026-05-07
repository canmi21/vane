# vane-core

Source: [`crates/core/`](../../crates/core/).

The foundation: types, traits, the symbolic IR, the compile pipeline. Knows nothing about hyper, quinn, wasmtime, or rustls. Owns no middleware or fetch implementations — only the shape of the compiled IR.

## Crate dependencies

`http`, `http-body`, `bytes`, `serde`, `serde_json`, `arc-swap`, `parking_lot`, `thiserror`, `tracing`, `async-trait`, `fancy-regex`, `ipnet` (with `serde`), `base64`, `sha2`, `tokio-util` (no default features, for `CancellationToken`), `rustls-pki-types` (pure-Rust shared types).

No async runtime executor dependency. Constructing and observing a `CancellationToken` works outside a tokio context; only `.cancelled().await` requires one, and that is the executor's concern. `vane lint` and `vane compile <DIR>` link only this crate.

## Owns

- **Core types** — `Body`, `Request`, `Response`, `L4Conn`, `ConnContext`, `Error`, `ErrorKind`, `FlowCtx`, `Transport`, `HttpVersion`. Source: `body.rs`, `conn_context.rs`, `error.rs`, `fetch.rs`, `flow_ctx.rs`, `l4.rs`.
- **Symbolic IR** — `SymbolicFlowGraph`, `Node`, ID newtypes, `SymbolicMiddlewareRef`, `SymbolicFetchRef`, `Terminator`, `PredicateInst`, `MiddlewareKind`, `FetchKind`, `FlowGraphMeta`. Source: `ir.rs`, `phase.rs`.
- **Metadata registry traits** — `MiddlewareMetadataProvider`, `FetchMetadataProvider`. Engine implements these and passes them into `compile`. Source: `metadata.rs`.
- **Compile pipeline** — `merge`, `expand`, `analyze`, `lower`, IR-level `validate`. Pure functions taking `RawRuleSet` + metadata providers, producing `Arc<SymbolicFlowGraph>`. Source: `compile/`.
- **Middleware traits** — `L4PeekMiddleware`, `L4BytesMiddleware`, `L7RequestMiddleware`, `L7ResponseMiddleware`, `Decision`, `ShortCircuit`. Source: `middleware.rs`.
- **Fetch traits** — `L7Fetch`, `L4Fetch`, `L7FetchOutput`, `Tunnel`. Source: `fetch.rs`.
- **`WasmRuntime` trait** — implementation lives in `vane-wasm`. Source: `wasm_runtime.rs`.
- **`FlowLogSink` trait + `FlowLogEvent` data** — concrete impl lives in `vane-engine`. Source: `flow_log.rs`.
- **Predicate** — `Predicate`, `CheckMap`, `Operator`, `Value` (config form); `PredicateInst`, `CompiledOperator`, `CompiledValue` (runtime form). Source: `predicate.rs`.
- **Preset expansion** — `port_forward`, `static_site`, `redirect_https`, `reverse_proxy` expand to `RawRule` bundles before merge. Source: `preset/`.
- **Config loader** — directory scan, dotenvy precedence, top-level merge. Source: `config/`.
- **Build / version metadata** — `BuildInfo`, project constants. Source: `meta.rs` (when present), `version.rs`.

## Types

L7 rides on `http::Request<Body>` / `http::Response<Body>` and `http_body::Body`. No custom request/response types.

```rust
pub enum Body {
    Static(bytes::Bytes),                                    // materialised, replayable
    Empty,                                                   // HEAD, 204, 304
    Stream(Pin<Box<dyn http_body::Body<...> + Send>>),       // hyper Incoming, H3Body, plugin output, CGI, ...
}
```

Three variants by design. All three implement `http_body::Body`. `Body::Stream` is `'static` — producers must own their data (most commonly via `Bytes`).

`http::Extensions` is the only typed escape hatch. No string-keyed KV. No `dyn Any` downcasts.

`ConnContext` is per-connection shared state, carried as `Arc<ConnContext>` in every request's extensions. H2 and H3 streams multiplexed on one connection share one `Arc`. `tls`, `peek`, and `user` use `parking_lot::Mutex<Option<_>>` for progressive population across phase transitions; `http_version` uses `OnceLock`. Refcount handles cleanup — no user-authored destructor.

`FlowCtx` is per-execution mutable state — one per executor invocation, owned on the executor stack. Carries `tracing::Span`, `Arc<dyn FlowLogSink>`, `CancellationToken`, `FlowLogVerbosity`, and the `TrajectoryBuilder` step accumulator. Fields are owned (no lifetime) so the struct survives `tokio::spawn` and `move` closures. `FlowCtx` deliberately does not carry a graph reference; routing is the executor's job.

H1 chunked, H2 DATA, and H3 DATA frames unify under `http_body::Body::poll_frame`. `BodyStreamAdapter` lets producers with foreign `Error` types land as `Body::Stream` — the `E: Into<Error>` bound means a one-line `From` impl plugs them in.

Vane's "no copy" guarantee: `Body::Stream`'s inner producer was already `Bytes`-shaped at the ingress parser (hyper for H1/H2, engine's `H3Body` for H3); vane neither copies nor accumulates these `Bytes` before handing to the upstream encoder. QUIC reassembly, H2 flow-control accounting, H1 chunked encoding are owned by `quinn` / `h3` / `hyper` internally — vane makes no claim about their internal costs.

## Predicate

Wire JSON: a single-key object whose key is a field path and whose value is an externally-tagged operator enum, plus the three combinators (`any_of`, `all_of`, `not`). Top-level `match` is implicit AND.

Combinator deserialisation is pure derive on `#[serde(untagged)]` enums; only `CheckMap` carries a one-line custom `Deserialize` that reads the map's only key as the path. Field paths come from a fixed closed set (`transport`, `remote.*`, `tls.*`, `http.method`, `http.uri.*`, `http.header.<name>`, `http.body`, `peek`); none of those collide with `any_of` / `all_of` / `not`, so no reserved-word policy.

Field paths are lowercase. The compiler suggests the lowercase form when an operator literal contains uppercase. SNI literals are rejected if they contain uppercase ASCII — the canonical comparison path is byte-for-byte; no `eq_ignore_ascii_case` shim.

Authoritative field-path table, operator × value-type compatibility, and inspection-level mapping live in `crates/core/src/predicate.rs`. `analyze` derives the inspection level (`L4-only < L4-peek < L7-header < L7-body`) used by `lower` for rule sorting.

`PredicateInst::test` receives a `PredicateView` — a phase-aware window. Reading state that does not exist in the current phase is a compile error rather than a runtime panic. Hash-consing is `Hash + Eq` cross-phase — same value domain, same lookup code; the validator's `(NodeId, Phase)` seen-set covers the rare shared-Check-across-phases case.

Regex uses `fancy-regex`. Pattern source ≤ 4 KiB; runtime backtrack limit is 1,000,000 steps. Patterns not using lookaround / backreferences delegate to the `regex` crate internally and run in linear time.

CIDR uses `ipnet::IpNet`. Mixing v4 and v6 inside `in` / `not_in` is allowed; `cidr` matches one family.

## Compile pipeline

```
RawRuleSet
  ↓ merge       crates/core/src/compile/merge.rs
MergedConfig
  ↓ expand      crates/core/src/compile/expand.rs
RawRuleSet
  ↓ analyze    crates/core/src/compile/analyze.rs
AnalyzedRuleSet
  ↓ lower      crates/core/src/compile/lower.rs
SymbolicFlowGraph
  ↓ validate   crates/core/src/compile/validate.rs
Arc<SymbolicFlowGraph>
```

See [`flow-model.md` § _Compile and link_](../flow-model.md#compile-and-link--two-stages-two-crates) for the full architectural picture, including the engine-side `link` step.

`merge` is deterministic: lex-sort files, stable-sort by `(order asc, filename lex)`, accumulate. Duplicate `rule` names are errors at merge. Global settings follow last-write-wins with a merge log. Output: `MergedConfig`, dumpable via `vane compile <DIR>`.

`expand` runs preset expansion before merge (preset emits `RawRule`s; the merge stage treats them like hand-written rules). The four MVP presets — `port_forward`, `static_site`, `redirect_https`, `reverse_proxy` — live in `preset/`. Each is a pure function `fn(args) -> Vec<RawRule>`. User-defined presets (via WASM or templates) are not supported; preset opinions belong at code-commit review, not config-load time.

`analyze` derives per-rule inspection level, specificity (predicate count), and LazyBuffer tracks. See [`flow-model.md` § _LazyBuffer_](../flow-model.md#lazybuffer).

`lower` groups by listener port, sorts by `(inspection level desc, specificity desc, name asc)`, builds a decision tree, flattens to `Vec<Node>`, hash-conses predicates and stateless middleware. Stateful middleware is per-call-site by construction.

`validate` checks IR integrity: ID resolution, DAG, phase machine, predicate-field legality. Failure aborts compile; no partial graph is exposed.

## Error type

Single crate-level type. Three layers stay strictly separate:

1. **Typed propagation** (`vane-core::Error` via `thiserror`) — internal functions return typed errors with rich kind + reason + source chain. No stringly-typed handling; no `anyhow` in library code.
2. **Structured tracing** (`tracing`) — every error production point emits a structured event with `kind` / `reason` / context fields. Flow log and structured log read these as machine-filterable data, not as parsed strings.
3. **Terminal-pretty display** (`anyhow`) — only `vane::main` and `vaned::main` wrap into `anyhow::Error` for stderr. Library code never imports `anyhow`.

Top-level `ErrorKind` is flat and stable (9 variants) for low-cardinality metric labels: `Io`, `Protocol`, `Upstream(UpstreamReason)`, `Middleware`, `Compile`, `Timeout(TimeoutKind)`, `Canceled`, `Resource(ResourceKind)`, `Internal`. Fine-grained distinctions live on the nested enums.

`Error::is_retryable()` is the single source of truth for which `(kind, reason)` combinations the retry loop honors. `Error::http_status()` maps to L7 status codes. `Error::source_chain()` walks `source()` to the root for flow-log encoding.

`SerializedError` is the `Clone + Serialize` POD shape for flow log fan-out; constructed once at emit time with size caps (`message` 4 KiB, `ctx` 1 KiB, `source_chain` 16 entries × 1 KiB) so a pathological TLS chain or deep WASM error cannot ship multi-MiB events to every subscriber.

`From<>` impls bridge `std::io::Error`, `hyper::Error`, `h3::Error`, `rustls::Error`, `fancy_regex::Error`, `serde_json::Error`, `ipnet::AddrParseError`, `hickory_resolver::ResolveError`, `tokio::time::error::Elapsed`. Each preserves the original as `source` for the chain.

## Config layers

Three layers separated by change frequency:

| Layer                | Location                          | Cadence           | Effect                  |
| -------------------- | --------------------------------- | ----------------- | ----------------------- |
| Deployment constants | `/etc/vaned/.env` (via `dotenvy`) | Deploy-time, rare | Daemon restart required |
| Daemon-scoped config | `/etc/vaned/config.json`          | Occasional        | Reload                  |
| Flow rules           | `/etc/vaned/rules/*.json`         | Frequent          | File-watch auto-reload  |

OS env wins over `.env` values. `.env` only fills variables not already set. `VANE_*` prefix; namespace prefixes (`SEC_`, `MGMT_`, `WASM_`) group related settings. Source: `config/`.

L1 security floor settings (`VANE_SEC_*`) are deploy-time constants — they describe daemon self-preservation, not the flows it serves. Floors are enforced at compile (a rule lowering a value below the floor fails with an explanatory error); raising values for high-traffic production is allowed.

ListenSpec grammar (transport prefix + address forms) lives at `crates/core/src/rule.rs`. Bare entries default to TCP for backwards compatibility; UDP listeners require the explicit `udp:` prefix. Wildcard port (`:0`) is rejected — graph entry keys must be stable.

Reload is whole-graph atomic. `Arc<FlowGraph>` swaps via `ArcSwap`; in-flight connections keep the captured Arc until completion. Old graph drops when its last user releases it. Per-listener or per-rule partial swap is deliberately unsupported — compile-time optimizations cross rule boundaries (shared predicate prefixes, LazyBuffer decisions).

## Rate limit (L2)

Built-in stateful internal middleware `rate_limit`. Token bucket only. Source: registered in `vane-engine`'s middleware factories; the L2 middleware impl lives at `crates/engine/src/middleware/rate_limit.rs`.

`window` is bounded `[1s, 60s]` at compile. Long-window rate limits cross into "needs persistent shared state" — that is application or sidecar concern, not proxy concern.

Key derivations: `RemoteIp`, `Header(name)`, `Cookie(name)`, `Query(name)`, `Composite(...)`, `Global`. Composing rule predicates with `KeyDerivation` produces "per X per path" / "global per path" / multi-tier shields naturally.

State is local-RAM `DashMap` only. Multi-daemon deployments behave as N independent limiters. Distributed rate limiting is application-layer concern.

The middleware's `DashMap<Key, TokenBucket>` lives on `Arc<FlowGraph>`. Reload resets — see [`flow-model.md` § _State migration_](../flow-model.md#state-migration-on-reload). For DDoS-class protection that must survive reloads, the L1 floor (daemon-scoped, not on the graph) is the right layer.

| Aspect          | L1 (security floor)               | L2 (`rate_limit`)                       |
| --------------- | --------------------------------- | --------------------------------------- |
| Position        | Listener / pre-handshake / parse  | FlowGraph middleware node               |
| Disable?        | No                                | Yes — opt-in per rule                   |
| Goal            | Keep `vaned` alive                | Smooth application-layer load           |
| Trigger outcome | Close / RST                       | 429 / custom response                   |
| State scope     | Per-IP / per-conn / daemon-global | Arbitrary dimension via `KeyDerivation` |
| Configuration   | Env vars via dotenvy              | Per-rule in `rules/*.json`              |
| Window          | Fixed per limit                   | 1–60 s (compile-checked)                |

L1 implementation lives in [`crates/engine.md` § _Security floor_](engine.md#security-floor).

## Listener kind derivation

`ListenerKind { Raw, Http, Auto }` is derived per listener address from the union of every rule's entry subgraph at compile. Operators do not write `listener_kind`; the graph shape is the single source of truth.

| Reachable terminator set                             | Derived kind |
| ---------------------------------------------------- | ------------ |
| Only L4 fetches (`L4Forward`, …)                     | `Raw`        |
| Only L7 fetches (every L4→L7 path crosses `Upgrade`) | `Http`       |
| Both reachable from one entry                        | `Auto`       |

Per-transport interpretation:

| Variant | TCP                                                       | UDP                                                                                                               |
| ------- | --------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------- |
| `Raw`   | All traffic is L4 byte forwarding                         | All traffic is L4 datagram forwarding (no QUIC awareness)                                                         |
| `Http`  | All traffic is H1.1 / H2 (TLS-terminated when cert bound) | All traffic is H3 — QUIC handshake terminated by `quinn`, H3 streams over it                                      |
| `Auto`  | Peek discriminates TLS, H1, H2, raw                       | Peek discriminates QUIC initial (→ H3), QUIC initial with `tls.sni` (→ L4Forward by SNI), other UDP (→ L4Forward) |

Operators declare wire transport via the `udp:` prefix on listen entries; listener kind follows from rules.

## Build info / project metadata

`vane-core::version::BuildInfo` is the shared shape both binaries fill from compile-time env vars (set by their own `build.rs`). `format_version` produces the four-block output (header / build / legal / links). Project metadata constants (`DESCRIPTION`, `COPYRIGHT`, `HOMEPAGE`, `REPOSITORY`, `LICENSE`, `LICENSE_URL`) live in this crate as the single source of truth used in `--version` output, CLI help, and generated docs.

## Tests

Integration tests in `crates/core/tests/` cover compile-pipeline shapes:

- `trajectory.rs` — `FlowTrajectory` builder and serde round-trip.
- `preset_pipeline.rs` — each MVP preset round-trips through `compile()` to a valid `Arc<SymbolicFlowGraph>`.
- `config_load.rs` — directory walk + dotenvy precedence; full `load → merge → expand → analyze → lower → validate` for a deployment-shaped tree.
- `acme_inject.rs` — lower's ACME HTTP-01 inject step lands in expected graph shape.
- `zero_rtt_compile.rs` — TLS 1.3 0-RTT compile-time constraints.

Tests that mutate process env are `#[serial]` (via `serial_test`). Rust 1.95 marks `std::env::set_var` `unsafe`; the `unsafe` is sound under serial execution because no other test thread reads env concurrently. The workspace `unsafe_code = "deny"` is relaxed in those test files only.
