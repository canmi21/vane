# Architecture

Reading order:

1. [`00-charter.md`](00-charter.md) — scope, non-goals, target user, MVP slice
2. [`01-topology.md`](01-topology.md) — processes, transports, filesystem layout
3. [`02-flow.md`](02-flow.md) — the funnel, rule compilation, pay-as-you-go
4. [`03-types.md`](03-types.md) — Request, Body, Extensions, ConnContext
5. [`04-middleware.md`](04-middleware.md) — 2×2 taxonomy, lifecycle
6. [`05-terminator.md`](05-terminator.md) — Fetch and Terminator (upstream contact + client write)
7. [`06-l4.md`](06-l4.md) — L4 listeners, udp_dispatch, l4_forward
8. [`07-l7.md`](07-l7.md) — HTTP any-bridge, WebSocket, upstream pool
9. [`08-tls.md`](08-tls.md) — TLS scenarios, cert boundary
10. [`09-config.md`](09-config.md) — JSON schema, merge, compile, HMR
11. [`10-management.md`](10-management.md) — shared protocol over two transports
12. [`11-wasm.md`](11-wasm.md) — wasmtime, pooling, ABI outline
13. [`13-rate-limit.md`](13-rate-limit.md) — L1 security floor + L2 user middleware
14. [`14-presets.md`](14-presets.md) — preset expansion stage, raw-rule transparency, preset catalog
15. [`15-cgi.md`](15-cgi.md) — CGI driver (process model, env, path handling, security)

## Status

First-draft architecture. The **shape** of each abstraction (types, taxonomies, boundaries) is the contract — those decisions converged through discussion and should be treated as load-bearing. **Concrete numeric values** (timeouts, pool sizes, buffer limits, filesystem paths) and **secondary naming** (enum variant names, operator aliases, verb names) are proposals open for revision.

## Firm architectural decisions

Decisions already agreed and baked into these docs. Changing any of these is a major-rewrite event:

- L7 types ride on `http::Request<B>` / `http::Response<B>` and `http_body::Body`. No custom request/response types.
- `http::Extensions` is the only typed escape hatch. No string-keyed KV, no `dyn Any`.
- Per-connection state is `Arc<ConnContext>`, shared across multiplexed streams, carried in each request's extensions.
- Configuration is JSON-only, multi-file merge, compile to immutable `FlowGraph`, hot-swapped via `ArcSwap`.
- FlowGraph validity (every path reaches a Terminator) is checked at compile, not at runtime.
- LazyBuffer is a load-time compilation decision, not a runtime check.
- Middleware taxonomy is 2×2: origin (internal Rust / external WASM) × state (stateless / stateful).
- Flow nodes have three roles: **Middleware** (decisions and state mutations), **Fetch** (upstream contact — produces Response or byte tunnel), **Terminator** (writes final output, closes connection). Fetch and Terminator are always built-in; never extensible. WASM extends decisions (Middleware), not actions.
- L7 data flow is: `Request middleware chain → Fetch → Response middleware chain → Terminator`. Response body is mutable via `L7ResponseMiddleware`.
- L4 / L7 is the only layer distinction. No L5, no L6.
- WebSocket is HTTP/1.1 only. Permanent scope decision.
- Two binaries: `vane` (CLI + TUI) and `vaned` (daemon).
- Management is one protocol, two transports: Unix socket (default) and HTTP-over-TCP (opt-in remote).
- WASM runtime is wasmtime; stateful plugins use a fixed-size instance pool; stateless plugins use `PoolingAllocator` reuse.

## Open questions blocking implementation

All architectural open questions from the initial draft have been resolved through discussion. Concrete numeric values (default timeouts, pool sizes, etc.) remain proposals that can be adjusted without architectural rework.
