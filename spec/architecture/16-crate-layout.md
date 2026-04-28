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
├── cli/           vane              ── binary: CLI + TUI (client of mgmt)
└── daemon/        vaned             ── binary: daemon (ties everything together)

tests/                               ── workspace-level integration tests
```

Folder names describe each crate's **role**; `package.name` is the **published brand**. The two binary packages (`vane`, `vaned`) keep their short public names while living in functional folders (`cli/`, `daemon/`); the library packages keep the `vane-*` prefix that matches their folder name one-to-one.

Directory names are short; crate names are `vane-*` prefixed (for clarity and eventual publishing).

## Crate responsibilities

### `vane-core`

The foundation and the **symbolic IR**. **Knows nothing about hyper / quinn / wasmtime / rustls** — and, critically, owns no middleware / Fetch **implementations**, only the shape of the compiled IR.

Owns:

- Types: `Request = http::Request<Body>`, `Response`, `Body` enum, `ConnContext`, `Error + ErrorKind`, `L4Conn`.
- Symbolic IR: `SymbolicFlowGraph`, `Node`, `NodeId / PredicateId / MiddlewareId / FetchId / TerminatorId`, `SymbolicMiddlewareRef`, `SymbolicFetchRef`, `Terminator` (unit enum), `PredicateInst`, `MiddlewareKind`, `FetchKind`, `BodySide`, `FlowGraphMeta`.
- Metadata registry traits: `MiddlewareMetadataProvider`, `FetchMetadataProvider`, `MiddlewareMetadata`, `FetchMetadata`. Engine implements these and passes them into `compile`; core never sees concrete impls.
- Compilation pipeline: `merge`, `expand` (preset expansion with string middleware refs), `analyze`, `lower`, IR-level `validate`. Pure functions taking `RawRuleSet` + metadata providers, producing `Arc<SymbolicFlowGraph>`.
- Middleware traits: `L4PeekMiddleware`, `L4BytesMiddleware`, `L7RequestMiddleware`, `L7ResponseMiddleware`, `Decision`, `ShortCircuit`.
- Fetch traits: `L7Fetch`, `L4Fetch`, `L7FetchOutput`.
- `WasmRuntime` trait (implementation lives in `vane-wasm`).
- `FlowCtx`, `PredicateView`.
- `FlowLogSink` trait + `FlowLogEvent` / `FlowLogKind` data types (concrete `FlowLogSink` impl lives in `vane-engine` at S1-29).

Dependencies: `http`, `http-body`, `bytes`, `serde`, `serde_json`, `arc-swap`, `parking_lot`, `thiserror`, `tracing`, `async-trait` (for dyn-compatible async middleware / Fetch traits — see `04-middleware.md` § _Async Send via `async_trait`_), `fancy-regex`, `ipnet` with the `serde` feature (so `IpNet` round-trips inside `CompiledOperator::Cidr` for `vane compile --dry-run`), `base64` (bytes-typed `CompiledValue` variants serialize as standard-alphabet base64 strings in the dry-run JSON), `sha2` (for the SHA-256 `FlowGraphMeta::version_hash`), `tokio-util` with `default-features = false` (for `tokio_util::sync::CancellationToken` — see `FlowCtx` in `03-types.md`; the `sync` module is always-compiled in current tokio-util, not feature-gated), `rustls-pki-types` (a pure-Rust, runtime-free shared-PKI-types crate — for `TlsInfo::peer_cert`; `vane-core` does **not** pull the full `rustls` crate).

**No async runtime executor dependency.** Core pulls `tokio-util`'s `CancellationToken` type (and its transitive `tokio` sync primitives), but core itself never calls `tokio::spawn`, never drives a runtime, never opens sockets. Constructing and observing a `CancellationToken` works outside a tokio context; only `.cancelled().await` requires one, and that's the executor's concern. No network stack. No TLS. No WASM. `vane lint` / `vane compile --dry-run` link only this crate and serialize its `SymbolicFlowGraph` output — neither needs hyper, rustls, wasmtime, or a tokio runtime. Minimal foot-gun surface; this crate should build in <5 seconds cold on a developer laptop.

### `vane-engine`

The **runtime and the linker**. Implements `MiddlewareInst` / `FetchInst`, "links" a symbolic graph into an executable one, owns the listener tasks, owns the executor.

Owns:

- Runtime IR: `FlowGraph` (the **linked** form that holds `Vec<MiddlewareInst>` and `Vec<FetchInst>` of trait objects); `MiddlewareInst` enum; `FetchInst` enum.
- Link pass: `FlowGraph::link(sym: Arc<SymbolicFlowGraph>, mw_factories, fetch_factories) -> Result<Arc<FlowGraph>, LinkError>`. Linking is where feature-availability rejection happens (a `SymbolicMiddlewareRef` for a kind the build disabled fails here, not in core).
- Factories: `MiddlewareFactories`, `FetchFactories` — registries mapping a `name` to a constructor `fn(args) -> Result<MiddlewareInst, _>`. Engine registers built-ins at startup; WASM factories come from `vane-wasm`.
- Metadata provider impls: concrete `MiddlewareMetadataProvider` / `FetchMetadataProvider` the daemon passes into core's `compile`. Stateless / `needs_body` / `kind` come from the same registry so compile-time analysis and link-time construction agree.
- Executor: the iterative walker from `02-flow.md`, dispatching on `MiddlewareInst` / `FetchInst::L4|L7`.
- Listeners: accept loop per `(transport, addr)`, bind retry, cancellation, drain — per `01-topology.md`.
- HTTP server integration: hyper for H1/H2, h3 for H3; `udp_dispatch` for QUIC session demux.
- Fetch implementations: `HttpProxy`, `HttpSynthesize`, `WebSocketUpgrade`, `L4Forward`.
- H3 body adapter: `H3Body` struct + `H3StreamSource` trait (unifies `h3::server::RequestStream` and `h3::client::RequestStream` under a single `http_body::Body` surface). Engine constructs `Body::Stream(Box::pin(H3Body::new(stream)))` at H3 ingress. Also the hyper-ingress bridge — each protocol-specific stream type wrapped in `Body::Stream` via a `Box::pin` adapter.
- Upstream pools: `TcpPool` (hyper-util Client wrapper), `QuicPool` (our h3 client manager); fingerprint-based sharing.
- TLS: cert resolver, cert store, cert populators (`StaticCertPopulator` + space for `ManagedCertPopulator`); `ClientConfig` fingerprint cache; `TicketKeyManager`.
- Built-in middleware impls: SNI match, host header match, path prefix, method match, protocol detect, rate limit, `forward_client_ip`, etc.
- DNS: `hickory-resolver` integration.
- `ArcSwap<FlowGraph>` holds the **linked** graph — that is the one accept loops and the executor read.

Dependencies: `vane-core` + `tokio`, `hyper`, `hyper-util`, `hyper-rustls`, `h3`, `h3-quinn`, `quinn`, `rustls`, `rustls-native-certs`, `tokio-rustls`, `hickory-resolver`, `dashmap`, `webpki`, `webpki-roots` (or system roots), `notify` + `notify-debouncer-full` (file watcher with debounce), `metrics` + `metrics-exporter-prometheus`, `instant-acme` (gated behind the `acme` feature; `acme-dns-cloudflare` feature pulls the Cloudflare DNS-01 module), `rand` (for session-ticket keys and similar), `libc` (for the CGI `pre_exec` closure's `setuid` / `setgid` / `setrlimit` syscalls, gated behind the `cgi` feature).

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
- Verb schemas: `compile_dry_run`, `reload`, `get_config`, `get_connections`, `tail_flow`, `tail_log`, `get_metrics`, `stats`, `shutdown`, `get_pools`, `get_upstreams`.
- `server` module: mounts onto a Unix socket or HTTP-over-TCP; `vaned` uses it.
- `client` module: typed client against the same verb set; `vane` CLI/TUI uses it.

Dependencies: `vane-core` + `tokio`, `hyper` (for HTTP transport), `serde_json`. **NDJSON over chunked is the chosen streaming mechanism** — `tokio-tungstenite` is deliberately not a dependency anywhere in the workspace (flagging so reviewers catch accidental imports).

### `vane-testutil`

Shared across integration tests. Not linked into release binaries.

Owns:

- Echo HTTP/TCP/UDP servers with auto-teardown.
- Free port allocator.
- Tracing init for tests (captures to an in-memory sink).
- `build_flow(rules)` helper that constructs `FlowGraph` for unit testing without disk I/O.
- Fixture certs (self-signed; for TLS test paths). Generated at **test runtime** by `rcgen` (pure-Rust X.509); bytes are never committed to the repo, so expiry-flake is structurally impossible. Each test's tmpdir gets a fresh CA + leaf.
- ACME test plumbing: `pebble()` spawns `letsencrypt/pebble` via `testcontainers` for HTTP-01 live tests; `mock_dns()` returns a `hickory-server`-backed in-process DNS mock for DNS-01 live tests. See `spec/architecture/08-tls.md` § _ACME challenge modes_.

Dependencies: `vane-core`, `tokio`, `hyper`, `rustls`, `rcgen`, `testcontainers`, `hickory-server`, `assert_cmd`, `predicates`.

Only used in `[dev-dependencies]` — never enters release build.

### `vane` (binary; folder `crates/cli/`)

User-facing terminal binary. Does **not** depend on `vane-engine`.

Owns:

- CLI entry point (`clap`), command dispatch.
- TUI shell (`ratatui`).
- Client wiring against `vane-mgmt`.
- `vane compile --dry-run` compiles via `vane-core` (no engine needed; outputs JSON).

Dependencies: `vane-core`, `vane-mgmt` + `clap`, `ratatui`, `crossterm`, `tokio`.

This crate must build fast (seconds). Deployment footprint can be a single statically-linked binary ~5–10 MiB.

### `vaned` (binary; folder `crates/daemon/`)

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
resolver = "3" # MSRV-aware resolver, requires Rust 1.84+
members = [
	"crates/cli", # binary package `vane`
	"crates/core",
	"crates/daemon", # binary package `vaned`
	"crates/engine",
	"crates/mgmt",
	"crates/testutil",
	"crates/wasm",
	"tests",
]

[workspace.package]
edition = "2024" # requires Rust 1.85+
rust-version = "1.95" # MSRV
license = "see LICENSE"

[workspace.lints.rust]
unsafe_code = "deny" # per-file `#[allow(unsafe_code)]` is required for CGI pre_exec; reviewed in audit
missing_docs = "warn"
unreachable_pub = "warn"

[workspace.lints.clippy]
all = { level = "warn", priority = -1 }
pedantic = { level = "warn", priority = -1 }
nursery = { level = "warn", priority = -1 }
# selectively allowed
module_name_repetitions = "allow"
missing_errors_doc = "allow"
missing_panics_doc = "allow"

[workspace.dependencies]
# Dependencies are added via `cargo add <name>` — no hand-pinned versions.
# Cargo.lock captures exact versions and is committed to the repo for
# reproducible builds. When bumping, use `cargo update -p <crate>` and commit
# the new Cargo.lock explicitly.

[profile.release]
opt-level = "z" # size-optimized
lto = true # fat LTO
codegen-units = 1 # single codegen unit for maximum optimization
strip = true # strip symbols
panic = "abort" # no unwinding; smaller, faster

[profile.dev]
opt-level = 0 # no optimization
codegen-units = 256 # high parallelism for fast builds
lto = false
strip = false # keep debug symbols
debug = "full"
panic = "unwind" # normal unwind for tests / debuggers
```

### Dependency management policy

- **All dependencies added via `cargo add <crate> -p <workspace-member>`**. No hand-pinning versions in `Cargo.toml`.
- **`Cargo.lock` is committed** — it captures exact resolved versions for reproducibility.
- **Bumping**: `cargo update -p <crate>` in CI or ad-hoc, then commit the updated lock.
- **Shared deps**: use `[workspace.dependencies]` for crates used by 2+ members; each member references via `dep = { workspace = true }`.

### `.cargo/config.toml` aliases

```toml
[alias]
c = "check --all-targets --workspace"
b = "build --all-targets --workspace"
t = "test --workspace"
fmt = "fmt --all"
lint = "clippy --workspace --all-targets -- -D warnings"
ci = "test --workspace --all-features"
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

MVP release targets are enumerated in § _Target tier matrix_ below. The short form: static binaries for all Tier 1 targets (gnu + musl on linux x86_64/aarch64, plus apple-darwin arm64); compile-only CI for Tier 2 (FreeBSD, i686, armv7). Source tarball of the workspace.

Container images (Docker / OCI) are built from the static musl binaries on Alpine or distroless base. That's tooling, not architecture — out of scope for this document.

## Target tier matrix

`vane` is **Unix-only** (Windows is a permanent non-goal — see `00-charter.md` § _Permanently out of scope_). Among Unix targets, support is tiered:

| Target                          | Crypto    | Tier            | Wasm feature       | Notes                                                                                                                                                                             |
| ------------------------------- | --------- | --------------- | ------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `x86_64-unknown-linux-gnu`      | aws-lc-rs | Tier 1          | on                 | Primary CI target — full `cargo test` runs here. aws-lc-rs pre-generated bindings. wasmtime Tier 1.                                                                               |
| `aarch64-unknown-linux-gnu`     | aws-lc-rs | Tier 1          | on                 | aws-lc-rs pre-generated bindings. wasmtime Tier 2.                                                                                                                                |
| `x86_64-unknown-linux-musl`     | aws-lc-rs | Tier 1          | on                 | Primary **static-binary** release target (edge / router deploy). aws-lc-rs builds via bindgen; `musl-gcc` or `clang` suffices.                                                    |
| `aarch64-unknown-linux-musl`    | aws-lc-rs | Tier 1          | on                 | Same. Container base is typically Alpine or distroless.                                                                                                                           |
| `aarch64-apple-darwin`          | aws-lc-rs | Tier 1          | on                 | **Local dev target** (see `spec/testing.md`). aws-lc-rs pre-generated bindings via `security-framework`. wasmtime Tier 2.                                                         |
| `x86_64-unknown-freebsd`        | aws-lc-rs | Tier 2          | **off by default** | CI runs `cargo check` only. `wasm` feature turned off in the default FreeBSD build — wasmtime is Tier 3 there. Operators who want WASM rebuild with `--features wasm`.            |
| `i686-unknown-linux-gnu`        | ring      | Tier 2          | **unavailable**    | Cranelift has no 32-bit native backend; Pulley interpreter is experimental. `wasm` feature is not offered. `aws-lc-rs` is technically supported but uses bindgen — prefer `ring`. |
| `armv7-unknown-linux-gnueabihf` | ring      | Tier 2          | **unavailable**    | Same rationale: no 32-bit wasmtime backend. `ring` is the portable default.                                                                                                       |
| Windows (any)                   | —         | **unsupported** | —                  | Permanent non-goal. `vaned` assumes Unix signals, Unix-domain sockets, fork+exec, capability-based port binding.                                                                  |

Tier meanings:

- **Tier 1** — a release artifact is produced; full `cargo test --workspace` runs in CI (on `x86_64-unknown-linux-gnu` specifically, with other Tier 1 targets validated via `cargo check`). Guaranteed to boot and serve.
- **Tier 2** — compile-only in CI (`cargo check --target <t> --features <reduced>`). No release artifact; no functional test. Builds today, may regress silently.

Authoritative upstream references: [aws-lc-rs Platform Support](https://aws.github.io/aws-lc-rs/platform_support.html) for crypto-backend target coverage; [Wasmtime Tiers of Support](https://docs.wasmtime.dev/stability-tiers.html) for wasm availability.

## musl build matrix

musl targets are **first-class** (Tier 1). The release pipeline produces musl static binaries as the primary artifact for edge / router deployments — a single self-contained file, glibc-free.

Dep-by-dep musl posture:

| Dep                                              | Musl-clean?            | Requires                           | Notes                                                                                                                                                                                                         |
| ------------------------------------------------ | ---------------------- | ---------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `rustls` 0.23 + `aws-lc-rs`                      | buildable, not trivial | `musl-gcc` or `clang` + bindgen    | First-class path. Needs a musl C compiler on the build host; `cross` image ships with it.                                                                                                                     |
| `rustls` 0.23 + `ring`                           | yes                    | nothing                            | Fallback for 32-bit targets and cross-compile hosts without musl cc.                                                                                                                                          |
| `hyper` 1.x / `hyper-util` / `hyper-rustls`      | yes                    | nothing                            | Pure Rust.                                                                                                                                                                                                    |
| `h3` / `h3-quinn` / `quinn` 0.11                 | yes                    | nothing                            | Pure Rust QUIC stack.                                                                                                                                                                                         |
| `hickory-resolver`                               | yes                    | nothing                            | Glibc-NSS-free by design — chosen specifically so musl static-link is clean.                                                                                                                                  |
| `notify` + `notify-debouncer-full`               | yes                    | inotify syscalls (linux-musl fine) | Pure Rust bindings over inotify / kqueue.                                                                                                                                                                     |
| `dashmap` / `parking_lot` / `arc-swap` / `tokio` | yes                    | nothing                            | Pure Rust.                                                                                                                                                                                                    |
| `instant-acme`, `rcgen`                          | inherits crypto choice | —                                  | Pure Rust otherwise.                                                                                                                                                                                          |
| `wasmtime` 26+                                   | **upstream Tier 3**    | Cranelift support per target       | Builds on `x86_64/aarch64-unknown-linux-musl`; wasmtime declares it Tier 3 (no CI commitment). `vane` Tier 1 status here covers vane's own test coverage; wasmtime runtime stability is the upstream's claim. |
| `libc` (CGI `pre_exec` syscalls)                 | yes                    | nothing                            | Syscall shape identical on musl.                                                                                                                                                                              |
| `testcontainers`                                 | N/A                    | Docker at test time                | `[dev-dependencies]` only; never in release build.                                                                                                                                                            |

Recommended musl build command for edge / router deploy:

```
cargo build --release --target aarch64-unknown-linux-musl -p vaned \
  --no-default-features --features "aws-lc-rs,h3,cgi,wasm"
```

For 32-bit targets (`i686`, `armv7`), drop `wasm` and switch crypto:

```
cargo build --release --target armv7-unknown-linux-gnueabihf -p vaned \
  --no-default-features --features "ring,h3,cgi"
```

For FreeBSD (wasm opt-in):

```
cargo build --release --target x86_64-unknown-freebsd -p vaned \
  --no-default-features --features "aws-lc-rs,h3,cgi"
# add ',wasm' if you know you want it on FreeBSD
```

## CI orchestration shape

**Deferred past Stage 1** — this section documents the _shape_ so Stage 2's compile-matrix has a target to aim at. `script/` does not exist yet; CI workflow files do not exist yet. When they land, they follow this shape.

Logic lives in **shell scripts under `script/`**, not in YAML. Rationale: CI should be locally testable. A CI failure the developer cannot reproduce on their laptop is a CI that wastes time.

```
script/
├── fmt.sh               # cargo fmt --all -- --check && dprint check
├── lint.sh              # cargo clippy --workspace --all-targets -- -D warnings
├── test.sh              # cargo test --workspace [--features ...]
├── check-target.sh      # $1 = triple; $2 = feature string; cargo check --target $1 --no-default-features --features "$2"
├── check-mutual-excl.sh # asserts `cargo build --features "aws-lc-rs,ring"` fails with the expected compile_error!
├── check-no-openssl.sh  # ! cargo tree --workspace 2>&1 | grep -q 'openssl-sys '
├── build-release.sh     # $1 = target; $2 = features; static-link flags; strip
└── commitlint.sh        # commitlint --from ${1:-origin/main}
```

Each script: ≤ 20 lines of bash, `set -euo pipefail`, echoes its own command, exits non-zero on first failure.

`Justfile` recipes are thin wrappers — one per script. Local dev invokes them directly (`just fmt`, `just check-target aarch64-unknown-linux-musl "aws-lc-rs,h3,cgi,wasm"`).

`.github/workflows/*.yml` (when it lands) stays orchestration-only — a workflow file that `just fmt` / `just lint` / `just test` plus a loop over the Target tier matrix's Tier 2 targets for `just check-target`. No logic in YAML.

Cross-compile toolchain: prefer native `cargo build --target <t>` with rustup targets installed. Use [`cross`](https://github.com/cross-rs/cross) only when the local host's glibc is newer than the deploy target's (musl has no such problem — musl cross-compiles clean with rust-native).

## Feature flags

## Feature flags

Naming follows ecosystem conventions — short, lowercase, single-word where possible. No prefixes; the underlying concept (crate name or protocol name) is clear enough.

### Per-crate feature flags

| Crate         | Feature               | Default | Purpose                                                                                           |
| ------------- | --------------------- | ------- | ------------------------------------------------------------------------------------------------- |
| `vane-engine` | `aws-lc-rs`           | on      | rustls crypto provider = aws-lc-rs; default on Tier 1; see § _Target tier matrix_                 |
| `vane-engine` | `ring`                | off     | rustls crypto provider = ring (mutually exclusive); the portable default on 32-bit Tier 2 targets |
| `vane-engine` | `h3`                  | on      | compile h3 + quinn for HTTP/3 support                                                             |
| `vane-engine` | `cgi`                 | on      | compile CGI fork-exec path (pulls `libc`)                                                         |
| `vane-engine` | `acme`                | off     | compile `ManagedCertPopulator` + `instant-acme` for ACME HTTP-01 and DNS-01                       |
| `vane-engine` | `acme-dns-cloudflare` | off     | pulls the Cloudflare DNS-01 provider module (requires `acme`)                                     |
| `vaned`       | `aws-lc-rs`           | on      | forwards to `vane-engine/aws-lc-rs`                                                               |
| `vaned`       | `ring`                | off     | forwards to `vane-engine/ring`                                                                    |
| `vaned`       | `h3`                  | on      | forwards to `vane-engine/h3`                                                                      |
| `vaned`       | `cgi`                 | on      | forwards to `vane-engine/cgi`                                                                     |
| `vaned`       | `acme`                | off     | forwards to `vane-engine/acme`                                                                    |
| `vaned`       | `acme-dns-cloudflare` | off     | forwards to `vane-engine/acme-dns-cloudflare`                                                     |
| `vaned`       | `wasm`                | on      | links `vane-wasm` (pulls wasmtime)                                                                |
| `vane` (bin)  | `tui`                 | on      | compiles ratatui + crossterm TUI code                                                             |

No feature flags on `vane-core`, `vane-wasm`, `vane-mgmt`, `vane-testutil` — they are always-on code.

### Crypto backend: `aws-lc-rs` vs `ring` (mutually exclusive)

Compile-time enforcement in `vane-engine`:

```rust
#[cfg(all(feature = "aws-lc-rs", feature = "ring"))]
compile_error!("`aws-lc-rs` and `ring` features are mutually exclusive — pick one");

#[cfg(not(any(feature = "aws-lc-rs", feature = "ring")))]
compile_error!("one of `aws-lc-rs` or `ring` must be enabled");

pub fn default_provider() -> Arc<rustls::crypto::CryptoProvider> {
    #[cfg(feature = "aws-lc-rs")]
    { rustls::crypto::aws_lc_rs::default_provider().into() }
    #[cfg(feature = "ring")]
    { rustls::crypto::ring::default_provider().into() }
}

pub fn install_default_provider() -> Result<(), Error> {
    default_provider()
        .install_default()
        .map_err(|_| Error::internal("crypto provider already installed"))
}

pub const BACKEND_NAME: &str = {
    #[cfg(feature = "aws-lc-rs")] { "aws-lc-rs" }
    #[cfg(feature = "ring")]      { "ring" }
};
```

Trade-offs:

|                    | `aws-lc-rs` (default)                                                                                                                     | `ring`                                |
| ------------------ | ----------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------- |
| Performance        | fast — AES-NI / SHA-NI / AVX-512                                                                                                          | slower — basic assembly only          |
| Build toolchain    | needs cmake + C compiler (BoringSSL)                                                                                                      | pure Rust + small asm, no C toolchain |
| FIPS 140-3         | optional                                                                                                                                  | not available                         |
| musl cross-compile | **Tier 1** — via `musl-gcc` / `clang` (the `cross` image ships it; a raw `rustup target add` host needs to provide the C compiler itself) | cleanest (pure Rust + asm)            |
| 32-bit targets     | possible but runs bindgen                                                                                                                 | preferred default                     |
| FreeBSD            | possible, runs bindgen                                                                                                                    | fallback                              |
| Pre-gen bindings   | `x86_64/aarch64-unknown-linux-gnu`, `x86_64-apple-darwin` — other Tier 1/2 targets invoke bindgen during build                            | N/A                                   |
| Binary size        | slightly larger                                                                                                                           | slightly smaller                      |

### Feature-off → rule compile-time rejection

When a feature is disabled in the build, rules referencing the disabled capability fail at **rule compile time** (inside `vane compile` or at `vaned` boot's compile pass), not at request dispatch:

- `!h3` + rule with `HttpUpstream::Tcp { version: Http3 }` → compile error: `"this binary was built without the 'h3' feature"`
- `!cgi` + rule with `HttpUpstream::Cgi { ... }` → compile error: `"this binary was built without the 'cgi' feature"`
- `!wasm` + rule referencing a WASM plugin → compile error: `"this binary was built without the 'wasm' feature"`

Compile-time rejection surfaces feature mismatches early — before the daemon bind listeners or before a deploy — rather than as ambiguous runtime 5xx's.

The rejection lives in `vane-engine`'s `validate` pass (see `02-flow.md` § _validate_), not in `vane-core`'s parser. `vane-core` is feature-flag-free and parses every legal raw rule regardless of which crypto / protocol / extension features the downstream binary was built with — so a shared tool that only reads configs (e.g., `vane lint`) can be built without pulling in wasmtime or quinn. Engine owns the "what did this binary compile in" truth and gates accordingly via `FlowGraphMeta::feature_set`.

### Management HTTP: runtime, not compile-time

The HTTP-over-TCP management transport is **not** feature-gated. `hyper` is already linked for proxy duties; feature-gating HTTP management saves zero compile time and zero binary size.

Instead, HTTP management is driven by env vars:

```
VANE_MGMT_UNIX=/var/run/vaned.sock     # default, always bound
VANE_MGMT_HTTP_BIND=127.0.0.1:4479     # unset = Unix only; set = additional HTTP binding
VANE_MGMT_HTTP_TOKEN=<bearer-hash>      # required when HTTP bind is non-loopback
VANE_MGMT_HTTP_TLS_CERT=/etc/vaned/mgmt.crt
VANE_MGMT_HTTP_TLS_KEY=/etc/vaned/mgmt.key
```

Unix socket is always bound. HTTP-over-TCP is opt-in per deployment.

### Build matrix examples

```
# Default production (Tier 1 target, e.g. x86_64-unknown-linux-gnu or *-musl)
cargo build --release -p vaned

# 32-bit Tier 2 target — no wasm, ring crypto
cargo build --release --target armv7-unknown-linux-gnueabihf -p vaned \
  --no-default-features --features "ring,h3,cgi"

# FreeBSD (Tier 2, wasm opt-in)
cargo build --release --target x86_64-unknown-freebsd -p vaned \
  --no-default-features --features "aws-lc-rs,h3,cgi"
#  add ',wasm' if you want it on FreeBSD

# Minimal HTTP/1.1 + HTTP/2 only (drop h3, wasm, cgi)
cargo build --release -p vaned --no-default-features --features "aws-lc-rs"

# CLI without TUI
cargo build --release -p vane --no-default-features
```

See § _Target tier matrix_ and § _musl build matrix_ above for the full grid.

## Binary CLIs

Both binaries use `clap` (derive API) for argument parsing. The surface is intentionally small — most behavior is file- and env-driven.

### `vaned`

The daemon has exactly two concerns: print build info, and start with a given config directory.

```
vaned --version          print build info and exit
vaned -v                 (alias)
vaned --help             print help and exit
vaned -h                 (alias)

vaned --config DIR       start the daemon with the given config directory
vaned -c DIR             (alias)
```

`--config` is **required** when starting. `vaned` does not probe for default paths (`/etc/vaned`, `~/.vaned`, etc.) — it refuses to guess. Running `vaned` with no arguments exits with an error and a hint:

```
error: no config directory specified
hint: `vaned --config /etc/vaned` (or wherever your config lives)
```

This forces explicit config placement in systemd units, Docker images, etc.

### `vane`

The terminal binary.

#### CLI subcommand layout

Two design rules:

1. **No hyphens in subcommand names.** Multi-word verbs become nested subcommand groups (`vane get config`), not kebab strings (`vane get-active-config`). Keystrokes stay short, tab completion stays clean.
2. **Flat for global actions, grouped for data/streams.** `ping` / `reload` / `shutdown` / `compile` are top-level — they are themselves verbs. `get` and `tail` are dispatch groups for snapshot / streaming reads.

```
vane --version | -v
vane --help    | -h

# Global actions
vane ping                  liveness check
vane stats                 daemon summary (uptime, graph hash, listener state)
vane reload                trigger reload of running daemon
vane shutdown              graceful drain + exit
vane compile <DIR>         dry-run compile; emit SymbolicFlowGraph JSON

# Snapshots (`get` group)
vane get config            active SymbolicFlowGraph as JSON
vane get connections       in-flight connections snapshot
vane get metrics           counter/gauge snapshot (default Prometheus text; `--json` for parsed)
vane get pools             stateful WASM pool status (post-MVP)
vane get upstreams         pooled upstream connections (post-MVP)

# Streams (`tail` group)
vane tail flow             subscribe to FlowLogEvent broadcast (NDJSON)
vane tail log              subscribe to structured tracing log (NDJSON)

# TUI
vane tui                   launch TUI (requires `tui` feature)
```

CLI subcommand → wire verb mapping is one-to-one and mechanical: `vane get config` calls `get_config`, `vane tail flow` calls `tail_flow`, etc. The CLI does not hide or rename verbs; it only nests.

## Startup sequence (`vaned`)

Strict order; any failure in steps 1–6 aborts with non-zero exit and a descriptive stderr message.

1. **Parse CLI args** via clap. `--config` resolves to a valid directory or the process exits with a usage error.
2. **Load environment variables**:
   - OS env first (whatever systemd / shell / container injects) — **takes precedence**.
   - Then `<config-dir>/.env` is attempted via `dotenvy`. Values in the file fill in variables **not already set** by OS env; they do not overwrite.
3. **Install crypto provider** — `vane_engine::crypto::install_default_provider()`. Must happen before any TLS code runs.
4. **Initialize tracing** — `tracing-subscriber`, level from `VANE_LOG_LEVEL` (default `info`), output to stderr (journald captures automatically under systemd).
5. **Scan and parse** `<config-dir>/config.json` and `<config-dir>/rules/*.json`.
6. **Expand / merge / analyze / lower / validate** (core) to produce `Arc<SymbolicFlowGraph>`, then **link** (engine) to produce the runtime `Arc<FlowGraph>`.
7. **Bind listeners** (per `01-topology.md`). Individual per-listener bind failures are logged but don't abort boot.
8. **Start management transports** — Unix socket always (`VANE_MGMT_UNIX`), HTTP-over-TCP only if `VANE_MGMT_HTTP_BIND` is set.
9. **Spawn file watcher** on `<config-dir>`, enter run loop.

## Build info and version strings

Both binaries share a version string format via a `vane-core::version` helper.

### Output format

Three blocks separated by blank lines:

1. **Header** — `Vane — <description>` then `Copyright (C) <year> <author>`.
2. **Build** — `Built:` / `Rust:` / `Cargo:` always; `Features:` / `Protocols:` added on `vaned`. Label column width fixed at 12 (longest label `Protocols:` is 10 chars, + 2 col gap).
3. **Legal** — three-line notice: `Copyright (C) <year> <author>`, `Released under the MIT License without restriction.`, and `This software comes with ABSOLUTELY NO WARRANTY.`
4. **Links** — `Homepage:` / `Source:` / `License:`, same 12-col label width.

`vane`:

```
Vane — A compact programmable proxy engine

Built:      <version> (<commit> <date>)
Rust:       <rustc-version-line>
Cargo:      <cargo-version-line>

Copyright (C) 2025 Canmi <t@canmi.icu>

Released under the MIT License without restriction.
This software comes with ABSOLUTELY NO WARRANTY.

Homepage:   https://vane.canmi.app
Source:     https://github.com/canmi21/vane
License:    https://opensource.org/licenses/MIT
```

`vaned`:

```
Vane — A compact programmable proxy engine

Built:      <version> (<commit> <date>)
Rust:       <rustc-version-line>
Cargo:      <cargo-version-line>
Features:   aws-lc-rs, h3, cgi, wasm
Protocols:  tcp, udp, quic, h1, h2, h3, ws, cgi

Copyright (C) 2025 Canmi <t@canmi.icu>

Released under the MIT License without restriction.
This software comes with ABSOLUTELY NO WARRANTY.

Homepage:   https://vane.canmi.app
Source:     https://github.com/canmi21/vane
License:    https://opensource.org/licenses/MIT
```

`<rustc-version-line>` is the whole trailing portion of `rustc --version` (without the `rustc` prefix) — e.g., `1.95.0 (59807616e 2026-04-14)`. Same shape for `cargo`. Captured at build time by `build.rs`.

Protocol names use short canonical forms: `h1` / `h2` / `h3` / `ws`. Display order is L4 transports first (`tcp`, `udp`, `quic`), then HTTP family (`h1`, `h2`, `h3`, `ws`), then `cgi` (when the feature is on).

### `build.rs` contract

Each binary crate (`vane`, `vaned`) has its own `build.rs` that emits compile-time env vars via `cargo:rustc-env=`:

| Env var           | Source                                                             |
| ----------------- | ------------------------------------------------------------------ |
| `VANE_COMMIT`     | `git rev-parse --short HEAD` (or `unknown` when not in a git tree) |
| `VANE_BUILD_DATE` | UTC build date in `YYYY-MM-DD`                                     |
| `VANE_RUSTC`      | `rustc --version` trimmed to `1.x.y`                               |
| `VANE_CARGO`      | `cargo --version` trimmed to `1.x.y`                               |

The binary's `main.rs` reads these via `env!("VANE_...")` at compile time and passes them into the shared formatter.

Package version (`CARGO_PKG_VERSION`) is set automatically by Cargo from `[workspace.package].version` inherited via `version.workspace = true` — no build.rs needed for the version number itself.

### Shared `BuildInfo` in `vane-core`

```rust
// crates/core/src/version.rs
pub struct BuildInfo {
    pub version:    &'static str,
    pub commit:     &'static str,
    pub build_date: &'static str,
    pub rustc:      &'static str,
    pub cargo:      &'static str,
    pub features:   &'static [&'static str],   // empty for `vane` binary; populated for `vaned`
    pub protocols:  &'static [&'static str],   // empty for `vane` binary; populated for `vaned`
}

pub fn format_version(info: &BuildInfo) -> String { /* produces the output above */ }
```

Each binary constructs `BuildInfo` from its own compile-time env (via `env!`) and from `cfg!(feature = "...")` introspection for features / protocols.

For `vaned`, the `features` and `protocols` slices are computed:

```rust
// crates/daemon/src/version.rs (sketch; package name is `vaned`)
fn enabled_features() -> &'static [&'static str] {
    const FEATURES: &[&str] = &[
        #[cfg(feature = "aws-lc-rs")] "aws-lc-rs",
        #[cfg(feature = "ring")]      "ring",
        #[cfg(feature = "h3")]        "h3",
        #[cfg(feature = "cgi")]       "cgi",
        #[cfg(feature = "wasm")]      "wasm",
    ];
    FEATURES
}

fn supported_protocols() -> &'static [&'static str] {
    const PROTOS: &[&str] = &[
        "http/1.1", "http/2", "websocket", "tcp", "udp",
        #[cfg(feature = "h3")]  "http/3",
        #[cfg(feature = "h3")]  "quic",
        #[cfg(feature = "cgi")] "cgi",
    ];
    PROTOS
}
```

## Project metadata

Constants in `vane-core::meta`:

```rust
pub const DESCRIPTION: &str = "A compact programmable proxy engine";
pub const COPYRIGHT:   &str = "Copyright (C) 2025 Canmi <t@canmi.icu>";
pub const HOMEPAGE:    &str = "https://vane.canmi.app";
pub const REPOSITORY:  &str = "https://github.com/canmi21/vane";
pub const LICENSE:     &str = "MIT";
pub const LICENSE_URL: &str = "https://opensource.org/licenses/MIT";
```

These are the single source of truth for these values; used in `--version` output, CLI help text, and any generated documentation.
