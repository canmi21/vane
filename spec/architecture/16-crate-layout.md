# Crate Layout

This document synthesizes all prior architectural decisions into a concrete Rust workspace structure. Reading this document should tell an implementer "what goes in which crate" without ambiguity.

## Topology: 7-crate workspace

```
crates/
├── core/          vane-core         ── foundation layer (types, traits, IR)
├── engine/        vane-engine       ── runtime layer (executor, listeners, pools, TLS)
├── wasm/          vane-wasm         ── WASM plugin layer (wasmtime, Component Model, host fns)
├── mgmt/          vane-mgmt         ── management protocol (wire format, server, client)
├── testutil/      vane-testutil     ── dev-only test helpers
├── vane/          vane              ── binary: CLI + TUI (client of mgmt)
└── vaned/         vaned             ── binary: daemon (ties everything together)

tests/                               ── workspace-level integration tests
```

Directory names are short; crate names are `vane-*` prefixed (for clarity and eventual publishing).

## Crate responsibilities

### `vane-core`

The foundation. **Knows nothing about hyper / quinn / wasmtime / rustls.**

Owns:

- Types: `Request = http::Request<Body>`, `Response`, `Body` enum, `ConnContext`, `Error + ErrorKind`, `L4Conn`.
- FlowGraph IR: `FlowGraph`, `Node`, `NodeId / PredicateId / MiddlewareId / FetchId / TerminatorId`, `MiddlewareInst`, `FetchInst`, `Terminator`, `PredicateInst`.
- Compilation pipeline: `merge`, `expand` (preset expansion with string middleware refs), `analyze`, `lower`, `validate`. Pure functions from source config to `Arc<FlowGraph>`.
- Middleware traits: `L4PeekMiddleware`, `L4BytesMiddleware`, `L7RequestMiddleware`, `L7ResponseMiddleware`, `Decision`, `ShortCircuit`.
- `WasmRuntime` trait (implementation lives in `vane-wasm`).
- `Ctx`, `FetchOutput`.

Dependencies: `http`, `http-body`, `bytes`, `serde`, `serde_json`, `arc-swap`, `parking_lot`, `thiserror`, `tracing`.

No async runtime dependency. No network stack. No TLS. No WASM. Minimal foot-gun surface; this crate should build in <5 seconds cold on a developer laptop.

### `vane-engine`

The runtime. Implements everything needed to **execute** a compiled FlowGraph against real sockets.

Owns:

- Executor: the iterative walker from `02-flow.md`, implementing `Node::Fetch` dual-output dispatch.
- Listeners: accept loop per `(transport, addr)`, bind retry, cancellation, drain — per `01-topology.md`.
- HTTP server integration: hyper for H1/H2, h3 for H3; `udp_dispatch` for QUIC session demux.
- Fetch implementations: `HttpProxy`, `HttpSynthesize`, `WebSocketUpgrade`, `L4Forward`.
- Upstream pools: `TcpPool` (hyper-util Client wrapper), `QuicPool` (our h3 client manager); fingerprint-based sharing.
- TLS: cert resolver, cert store, cert populators (`StaticCertPopulator` + space for `ManagedCertPopulator`); `ClientConfig` fingerprint cache; `TicketKeyManager`.
- Built-in middleware: SNI match, host header match, path prefix, method match, protocol detect, rate limit, `forward_client_ip`, etc.
- Middleware registry: resolves string names (from preset expansion) to concrete `MiddlewareInst`.
- DNS: `hickory-resolver` integration.

Dependencies: `vane-core` + `tokio`, `hyper`, `hyper-util`, `h3`, `quinn`, `rustls`, `rustls-native-certs`, `tokio-rustls`, `hickory-resolver`, `dashmap`, `fancy-regex`, `webpki`, `webpki-roots` (or system roots), `notify` (for file watcher).

### `vane-wasm`

WASM plugin runtime. Separated from `engine` so `engine` can build and test without wasmtime.

Owns:

- `WasmtimeRuntime: vane_core::WasmRuntime`.
- Component Model loading (wit-bindgen host side), `get-metadata` invocation, metadata caching.
- Instance pools: `PoolingAllocator` config for stateless plugins; fixed-size pools for stateful.
- Host function implementations: `log`, `now-unix-ms`, `random`, `metric-counter`, `metric-gauge`, `http-fetch`.
- `http-fetch` routed back through `vane-engine`'s `TcpPool` via a trait (so `wasm` doesn't depend on `engine` directly).

Dependencies: `vane-core` + `wasmtime`, `wasmtime-wasi`, `wit-bindgen`, `bytes`.

### `vane-mgmt`

Management protocol — one wire format, two transports, shared by daemon and CLI.

Owns:

- Wire format: `Request` / `Response` / `Stream` frame shapes, JSON-over-line and JSON-over-HTTP serialization.
- Verb schemas: `compile_dry_run`, `reload`, `get_active_config`, `list_connections`, `tail_flow_log`, `tail_log`, `get_metrics`, `stats`, `shutdown`, `list_wasm_pools`, `list_upstreams`.
- `server` module: mounts onto a Unix socket or HTTP-over-TCP; `vaned` uses it.
- `client` module: typed client against the same verb set; `vane` CLI/TUI uses it.

Dependencies: `vane-core` + `tokio`, `hyper` (for HTTP transport), `serde_json`, `tokio-tungstenite` if streaming needs it (probably not — NDJSON over chunked works).

### `vane-testutil`

Shared across integration tests. Not linked into release binaries.

Owns:

- Echo HTTP/TCP/UDP servers with auto-teardown.
- Free port allocator.
- Tracing init for tests (captures to an in-memory sink).
- `build_flow(rules)` helper that constructs `FlowGraph` for unit testing without disk I/O.
- Fixture certs (self-signed; for TLS test paths).

Dependencies: `vane-core`, `tokio`, `hyper`, `rustls`.

Only used in `[dev-dependencies]` — never enters release build.

### `vane` (binary)

User-facing terminal binary. Does **not** depend on `vane-engine`.

Owns:

- CLI entry point (`clap`), command dispatch.
- TUI shell (`ratatui`).
- Client wiring against `vane-mgmt`.
- `vane compile --dry-run` compiles via `vane-core` (no engine needed; outputs JSON).

Dependencies: `vane-core`, `vane-mgmt` + `clap`, `ratatui`, `crossterm`, `tokio`.

This crate must build fast (seconds). Deployment footprint can be a single statically-linked binary ~5–10 MiB.

### `vaned` (binary)

The daemon. Glue between all library crates.

Owns:

- `main()`: env loading (`dotenvy`), logger setup (`tracing-subscriber`), config directory scan, initial compile, listener startup, WASM runtime construction, management server mount.
- Dependency injection: constructs `Arc<dyn WasmRuntime>` from `vane-wasm`, passes to `vane-engine::Engine`.
- Signal handling: SIGTERM (drain), SIGHUP (reload), SIGINT (immediate close).

Dependencies: `vane-core`, `vane-engine`, `vane-wasm`, `vane-mgmt`, plus `tokio`, `dotenvy`, `tracing-subscriber`.

## Dependency graph

Strict DAG, one direction only:

```
                     ┌──────┐
                     │ core │
                     └──┬───┘
       ┌────────────┬───┴────┬──────────┐
       │            │        │          │
   ┌───▼────┐   ┌───▼────┐ ┌─▼────┐   ┌─▼────┐
   │ engine │   │  wasm  │ │ mgmt │   │ testutil
   └───┬────┘   └───┬────┘ └──┬───┘   └──────┘
       │            │         │
       │            │         ├────────────────┐
       │            │         │                │
       ├────────────┘         │                │
       │                      │                │
   ┌───▼──┐               ┌───▼──┐
   │vaned │               │ vane │
   └──────┘               └──────┘
```

Enforced by CI: `cargo check --workspace` detects any inadvertent dependency cycle or inversion.

## Key boundary decisions

### WASM via trait injection

`vane-core` declares `trait WasmRuntime`. `vane-engine` depends on the **trait**, stored as `Arc<dyn WasmRuntime>`. `vane-wasm` provides the implementation. `vaned` wires them together at startup.

Result: `cargo build -p vane-engine` does not pull wasmtime. Cold build on engine alone: ~15 seconds. Full daemon cold build with wasmtime: ~60+ seconds.

### Preset expansion in core with string middleware references

`vane-core::expand()` emits `RawRule` values where `middleware` fields are **strings** (e.g., `"forward_client_ip"`, `"rate_limit"`). `vane-core::compile()` takes a `MiddlewareRegistry` (populated by `vane-engine` for built-ins and `vane-wasm` for WASM plugins) to resolve strings to concrete `MiddlewareInst`.

Result: `vane compile --dry-run` runs entirely inside `vane-core` for the emission, using a registry that lists middleware names without needing engine code loaded. Output is deterministic JSON.

### `vane` binary does not link engine

CLI and TUI are pure **clients** of the management protocol. They need:

- `vane-core` for types (to display compiled output, parse config)
- `vane-mgmt` for client-side protocol

They do **not** need `vane-engine` (no listener, no pool, no executor in this binary). Compile time and binary size both benefit.

## Workspace configuration

### Root `Cargo.toml`

```toml
[workspace]
resolver = "3"                                   # MSRV-aware resolver, requires Rust 1.84+
members  = [
  "crates/core",
  "crates/engine",
  "crates/wasm",
  "crates/mgmt",
  "crates/testutil",
  "crates/vane",
  "crates/vaned",
  "tests",
]

[workspace.package]
edition      = "2024"                            # requires Rust 1.85+
rust-version = "1.95"                            # MSRV
license      = "see LICENSE"

[workspace.lints.rust]
unsafe_code        = "forbid"                    # stricter than deny; cannot be overridden via allow
missing_docs       = "warn"
unreachable_pub    = "warn"

[workspace.lints.clippy]
all                    = { level = "warn", priority = -1 }
pedantic               = { level = "warn", priority = -1 }
nursery                = { level = "warn", priority = -1 }
# selectively allowed
module_name_repetitions = "allow"
missing_errors_doc      = "allow"
missing_panics_doc      = "allow"

[workspace.dependencies]
# Dependencies are added via `cargo add <name>` — no hand-pinned versions.
# Cargo.lock captures exact versions and is committed to the repo for
# reproducible builds. When bumping, use `cargo update -p <crate>` and commit
# the new Cargo.lock explicitly.

[profile.release]
opt-level     = "z"                              # size-optimized
lto           = true                             # fat LTO
codegen-units = 1                                # single codegen unit for maximum optimization
strip         = true                             # strip symbols
panic         = "abort"                          # no unwinding; smaller, faster

[profile.dev]
opt-level     = 0                                # no optimization
codegen-units = 256                              # high parallelism for fast builds
lto           = false
strip         = false                            # keep debug symbols
debug         = "full"
panic         = "unwind"                         # normal unwind for tests / debuggers
```

### Dependency management policy

- **All dependencies added via `cargo add <crate> -p <workspace-member>`**. No hand-pinning versions in `Cargo.toml`.
- **`Cargo.lock` is committed** — it captures exact resolved versions for reproducibility.
- **Bumping**: `cargo update -p <crate>` in CI or ad-hoc, then commit the updated lock.
- **Shared deps**: use `[workspace.dependencies]` for crates used by 2+ members; each member references via `dep = { workspace = true }`.

### `.cargo/config.toml` aliases

```toml
[alias]
c    = "check --all-targets --workspace"
b    = "build --all-targets --workspace"
t    = "test --workspace"
fmt  = "fmt --all"
lint = "clippy --workspace --all-targets -- -D warnings"
ci   = "test --workspace --all-features"
```

## Tests

Integration tests live in the workspace-level `tests/` crate (`tests/Cargo.toml`), mirroring rev-2's pattern. Organized by concern:

```
tests/
├── Cargo.toml        # [package] name = "vane-tests"  (dev-only)
├── src/
│   └── lib.rs        # minimal, just to let Cargo treat tests/ as a crate
├── tests/
│   ├── engine_compile.rs    # FlowGraph compilation end-to-end
│   ├── engine_l4.rs         # L4 forwarding behavior
│   ├── engine_l7.rs         # L7 proxy behavior (incl. any-bridge)
│   ├── engine_tls.rs        # TLS termination / SNI / upstream TLS
│   ├── wasm_plugin.rs       # WASM plugin loading + invocation
│   ├── mgmt_protocol.rs     # management API verbs
│   ├── rate_limit.rs        # L1 + L2 rate limiting
│   └── presets.rs           # preset expansion correctness
```

All integration test files reference `vane-testutil` for fixtures. Unit tests stay in-crate as `#[cfg(test)] mod tests` (per `spec/testing.md`).

## Release artifacts

MVP release targets:

- **`vane`** static binary for: `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`, `x86_64-apple-darwin`, `aarch64-apple-darwin`.
- **`vaned`** static binary for same targets.
- Source tarball of the workspace.

Building with musl static-link confirms the hickory-resolver DNS choice (glibc NSS is not involved; see `07-l7.md`).

Container images (Docker / OCI) are built from the static binaries on Alpine or distroless base. That's tooling, not architecture — out of scope for this document.
