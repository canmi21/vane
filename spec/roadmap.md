# Implementation Roadmap

Three stages, dependency-ordered. This document is the **index** into the architecture spec from an implementation perspective — it does not add any behavioral claim of its own; everything here is a pointer back to `spec/architecture/`.

Time is measured in **features + dependency waves**, not calendar weeks. Agents complete mechanical work faster than human estimates suggest, and the human role in this project is architecture discussion plus review, not coding throughput. A "wave" is a set of features whose `depends_on` are all in earlier waves; a wave can be executed in parallel if context allows.

## Stages

- **Stage 1** — Typed skeleton + H1 reverse-proxy MVP. 33 features. Zero TLS, zero H2/H3, zero WASM; plaintext H1 + L4 forward + Unix-socket management + hot reload + L1 security floor. Delivers a usable reverse proxy with the full IR pipeline underneath.
- **Stage 2** — TLS + H2 + WebSocket + L2 rate-limit + HTTP-transport management + Prometheus metrics. 20 features. Production-serviceable slice: everything an operator needs for a TLS reverse proxy with observability.
- **Stage 3** — H3 + WASM + CGI + ACME + mTLS + TLS 1.3 0-RTT + TUI. 18 features. Full scope from `00-charter.md`.

Feature IDs are stable across the project's life — refer to them in commits, issues, test fixtures.

## Stage 1 — Typed skeleton + H1 reverse-proxy MVP

| ID     | Feature                                                                                                                                         | Crate provision                                                   | Depends on          |
| ------ | ----------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------- | ------------------- |
| S1-01  | Workspace scaffold (7 crates, resolver=3, edition=2024, MSRV 1.95)                                                                              | `cargo` workspace                                                 | —                   |
| S1-02  | `Error` + `ErrorKind` + reason enums + `From<>` bridges                                                                                         | `thiserror`                                                       | S1-01               |
| S1-03  | `meta` constants + `BuildInfo` + `build.rs` in both binaries                                                                                    | `std`                                                             | S1-01               |
| S1-04  | Core types: `Body`, `Request`/`Response`, `L4Conn`, `ConnContext` (with `ConnId`), `FlowCtx`, `Transport`, `HttpVersion`                        | `http`, `http-body`, `bytes`, `parking_lot`, `tokio` (types only) | S1-02               |
| S1-05  | Middleware + Fetch traits (`L4Peek`/`L4Bytes`/`L7Req`/`L7Resp`/`L7Fetch`/`L4Fetch`, `Decision`, `Tunnel`)                                       | `async-trait` 0.1                                                 | S1-04               |
| S1-06  | `PredicateInst` + `CompiledOperator`/`CompiledValue` + `PredicateView`                                                                          | `fancy-regex`, `ipnet`                                            | S1-02, S1-04        |
| S1-07  | Rule JSON parse (`Predicate` untagged, `CheckMap` custom deserialize, `RawRule`, `MiddlewareRef`, `TerminateSpec`, `SourceInfo`)                | `serde`, `serde_json`                                             | S1-06               |
| S1-08  | `MiddlewareMetadataProvider` + `FetchMetadataProvider` traits                                                                                   | pure trait                                                        | S1-05               |
| S1-09  | Core compile pipeline: `merge`, `expand` (preset stubs), `analyze`, `lower`, IR `validate`                                                      | pure Rust                                                         | S1-07, S1-08        |
| S1-10  | `SymbolicFlowGraph` + `Node` + `*Id` newtypes + `FlowGraphMeta` (SHA-256 version_hash)                                                          | `sha2` 0.10                                                       | S1-09               |
| S1-11  | Phase state machine transition table (const lookup)                                                                                             | `std`                                                             | S1-10               |
| S1-12  | `vane-engine::FlowGraph` + `MiddlewareInst` + `FetchInst` + `FlowGraph::link` (feature-gate rejection)                                          | `vane-core`, `arc-swap`                                           | S1-10               |
| S1-13  | TCP listener loop (accept + backoff-bind + SO_REUSEADDR + cancellation + soft drain)                                                            | `tokio::net`, `tokio_util::sync::CancellationToken`               | S1-12               |
| S1-14  | Dual-stack IPv4/IPv6 expansion + `VANE_BIND_IPV{4,6}` env toggles                                                                               | `std::net`                                                        | S1-13               |
| S1-15  | Executor iterative walker (owned-slots state machine, Decision routing, default fallback tombstones)                                            | `tokio`                                                           | S1-12, S1-05        |
| S1-16  | `protocol_detect` L4 middleware (peek ≤ 8 KiB, HTTP/1.x prefix + H2 preface)                                                                    | `bytes`, `memchr`                                                 | S1-15               |
| S1-17  | `Upgrade` node + L4→L7 bridge (hand TCP stream to `hyper::server::conn::http1::Builder`)                                                        | `hyper` 1.x, `hyper-util::rt::TokioIo`                            | S1-15, S1-16        |
| S1-18  | `L4ForwardFetch`                                                                                                                                | `tokio::io::copy_bidirectional`                                   | S1-15               |
| S1-19  | `HttpProxyFetch` H1→H1 path                                                                                                                     | `hyper_util::client::legacy::Client`                              | S1-17               |
| S1-20  | `HttpSynthesizeFetch`                                                                                                                           | —                                                                 | S1-17               |
| S1-21  | Built-in L7 stateless middleware: `host_header_match`, `path_prefix`, `method_match`, `forward_client_ip`                                       | `http::HeaderMap`                                                 | S1-17               |
| S1-22  | Preset expansion (full): `port_forward`, `reverse_proxy` (without WS gate), `static_site`, `redirect_https`                                     | pure Rust                                                         | S1-09, S1-21        |
| S1-23  | Terminator impls: `WriteHttpResponse` (H1 encoder + chunked/Content-Length decision), `ByteTunnel`                                              | `hyper`                                                           | S1-18, S1-19, S1-20 |
| S1-24  | `vane-mgmt` wire format + Unix line-delimited JSON transport                                                                                    | `tokio::net::UnixListener`, `serde_json`                          | S1-02               |
| S1-25  | `vane-mgmt` MVP verbs: `compile_dry_run`, `reload`, `get_config`, `stats`, `shutdown`, `get_connections`                                        | —                                                                 | S1-24, S1-12        |
| S1-26  | Config loader: scan dir, merge, `.env` via `dotenvy` (OS env wins)                                                                              | `dotenvy`                                                         | S1-09, S1-26a       |
| S1-26a | `.env` schema for `VANE_*` vars                                                                                                                 | `dotenvy`                                                         | S1-02               |
| S1-27  | File watcher + debounce                                                                                                                         | `notify` 6.x, `notify-debouncer-full` 0.3+                        | S1-26               |
| S1-28  | Hot reload via `ArcSwap` (skip store when version_hash unchanged)                                                                               | `arc-swap` 1.7                                                    | S1-12, S1-27        |
| S1-29  | `tracing` init + `FlowLogSink` (broadcast channel, fan-out)                                                                                     | `tracing`, `tracing-subscriber`, `tokio::sync::broadcast`         | S1-02               |
| S1-30  | L1 security floor (per-IP + global caps, header/body timeouts, floor-enforcement at compile)                                                    | `dashmap`, `tokio::time`, `std::sync::atomic`                     | S1-13, S1-26a       |
| S1-31  | `vaned` main: crypto-provider install (stub ok, no TLS this stage), tracing, config scan, compile+link, listeners, mgmt socket, signal handlers | `tokio::signal`, `clap` 4                                         | S1-28, S1-25, S1-29 |
| S1-32  | `vane` CLI: `clap` derive, `compile <DIR>`, `reload`, `--json` / pretty output                                                                  | `clap`, `vane-core`, `vane-mgmt` client                           | S1-09, S1-24        |
| S1-33  | `vane-testutil` baseline: tracing sink, free-port allocator, echo HTTP/TCP, `build_flow` helper, `VanedFixture` scaffold                        | `hyper`, `tokio` (TLS fixtures stubbed until S2)                  | S1-12, S1-17        |

## Stage 2 — TLS + H2 + WS + L2 rate-limit + HTTP mgmt + Prometheus

| ID    | Feature                                                                                                                                           | Crate provision                                    | Depends on          |
| ----- | ------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------- | ------------------- |
| S2-01 | Crypto-provider install (`aws-lc-rs` default, `ring` alternative)                                                                                 | `rustls::crypto` 0.23                              | S1-31               |
| S2-02 | `CertStore` + `CertEntry` + `VaneCertResolver : ResolvesServerCert`                                                                               | `rustls` 0.23                                      | S2-01               |
| S2-03 | `CertPopulator` trait + `StaticCertPopulator`                                                                                                     | `rustls-pemfile`, `rustls::pki_types`              | S2-02               |
| S2-04 | Listener TLS termination (peek via `rustls::server::Acceptor`, handshake via `tokio-rustls::TlsAcceptor`, SNI lowercase, ALPN per `ListenerKind`) | `tokio-rustls` 0.26, `rustls` 0.23                 | S2-02, S1-17        |
| S2-05 | H2 server ingress (post-TLS ALPN `h2`)                                                                                                            | `hyper::server::conn::http2`                       | S2-04               |
| S2-06 | `HttpProxyFetch` H1/H2 upstream with TLS connector                                                                                                | `hyper-rustls` 0.27+, `hyper-util`                 | S2-04, S1-19        |
| S2-07 | `TlsConfigFingerprint` cache (CRL source-identity hashing)                                                                                        | `dashmap`, `rustls`                                | S2-06               |
| S2-08 | `hickory-resolver` DNS integration with per-upstream nameserver override                                                                          | `hickory-resolver`                                 | S2-06               |
| S2-09 | SNI peek middleware (real; pre-handshake ClientHello parse)                                                                                       | `rustls::server::Acceptor` low-level               | S2-02               |
| S2-10 | `WebSocketUpgradeFetch` (H1.1 ↔ H1.1 byte tunnel, bi-outcome 101 vs 4xx)                                                                          | `hyper::upgrade`                                   | S2-05               |
| S2-11 | L4 UDP listener + `L4Forward { transport: Udp }` + 5-tuple session forwarder                                                                      | `tokio::net::UdpSocket`                            | S1-13               |
| S2-12 | `L7ResponseMiddleware` execution path + response-side LazyBuffer trigger                                                                          | —                                                  | S1-15               |
| S2-13 | L2 `rate_limit` middleware (token bucket, `KeyDerivation`, 1-60 s window)                                                                         | `dashmap`, `tokio::time::Instant`                  | S1-15, S1-09        |
| S2-14 | `vane-mgmt` HTTP-over-TCP transport (bearer + TLS-required-non-loopback)                                                                          | `hyper` 1.x, `tokio-rustls`                        | S1-24, S2-01        |
| S2-15 | Streaming mgmt verbs: `tail_flow`, `tail_log`                                                                                                     | `tokio::sync::broadcast`                           | S1-29, S2-14        |
| S2-16 | Prometheus metrics + `get_metrics` verb (format: prometheus\|json)                                                                                | `metrics` 0.24, `metrics-exporter-prometheus` 0.16 | S2-14               |
| S2-17 | Retry inside `HttpProxyFetch` (is_retryable table, idempotent-method gate)                                                                        | `tokio::time::sleep`                               | S2-06               |
| S2-18 | `ListenerKind::Auto` protocol detection (extends S1-16 with TLS peek)                                                                             | `rustls::server::Acceptor`                         | S2-04, S2-05, S1-16 |
| S2-19 | `TicketKeyManager` rotation (ArcSwap current+previous)                                                                                            | `rustls::server::ProducesTickets`                  | S2-01               |
| S2-20 | `vane` CLI: nested subcommand layout (`get` / `tail` groups + flat actions); `get metrics` is the final piece                                     | `vane-mgmt` client                                 | S1-32, S2-15, S2-16 |

## Stage 3 — H3 + WASM + CGI + ACME + mTLS + 0-RTT + TUI

| ID    | Feature                                                                                                                          | Crate provision                                                           | Depends on   |
| ----- | -------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------- | ------------ |
| S3-01 | Engine `H3Body` + `H3StreamSource` impls + H3 server path (engine wraps `H3Body` in `Body::Stream`)                              | `quinn` 0.11, `h3` 0.0.6+, `h3-quinn` 0.0.7+                              | S2-04        |
| S3-02 | H3 upstream in `HttpProxyFetch` + `QuicPool`                                                                                     | `h3`, `h3-quinn`, `quinn`                                                 | S3-01, S2-06 |
| S3-03 | `udp_dispatch` QUIC session demultiplexer                                                                                        | `quinn` (shared `UdpSocket`)                                              | S3-01, S2-11 |
| S3-04 | `WasmtimeRuntime : WasmRuntime` + Component Model load (sync host, async bindings)                                               | `wasmtime` 26+, `wit-bindgen` 0.35+                                       | S1-05        |
| S3-05 | WASM instance pools (`PoolingAllocator` for stateless, fixed-size for stateful)                                                  | `wasmtime::PoolingAllocatorConfig`                                        | S3-04        |
| S3-06 | WASM host functions (`log`, `now-unix-ms`, `random`, `metric-*`, `http-fetch` via `HttpFetchBackend` trait)                      | `rand`, `wasmtime-wasi`                                                   | S3-05, S2-16 |
| S3-07 | WASM module hot-reload (metadata-same = swap Component only; metadata-changed = recompile graph)                                 | —                                                                         | S3-05, S1-28 |
| S3-08 | CGI path in `HttpProxyFetch` via `HttpUpstream::Cgi` (`tokio::process::Command` + `CommandExt::pre_exec` for setuid/gid/rlimits) | `tokio::process`, `libc`                                                  | S2-17        |
| S3-09 | `ManagedCertPopulator` (ACME) via `instant-acme` — HTTP-01 + DNS-01                                                              | `instant-acme` 0.7+, Cloudflare DNS-01 gated behind `acme-dns-cloudflare` | S2-03        |
| S3-10 | OCSP fetch via cert's AIA URL (both populators)                                                                                  | `x509-parser`, hyper-based fetch                                          | S2-03        |
| S3-11 | CRL fetch + `rustls::WebPkiCrlProvider`                                                                                          | `rustls::WebPkiCrlProvider`                                               | S2-06        |
| S3-12 | Listener mTLS (`ClientAuth::Request` / `Require`, `ClientTrustStore`)                                                            | `rustls` server client-cert verifier                                      | S2-04        |
| S3-13 | TLS 1.3 0-RTT (per-rule `allow_0rtt`, compile-gate to idempotent methods)                                                        | `rustls` 0.23                                                             | S2-04        |
| S3-14 | WASM plugin integration with FlowGraph (`MiddlewareInst::Wasm`)                                                                  | —                                                                         | S3-04, S1-12 |
| S3-15 | `get_pools` + `get_upstreams` mgmt verbs                                                                                         | —                                                                         | S2-15, S3-05 |
| S3-16 | TUI (`ratatui` + `crossterm`, pure view-state machine underneath)                                                                | `ratatui` 0.29+, `crossterm` 0.28+                                        | S2-20        |
| S3-17 | `http.body` predicate full integration into `PredicateView::L7Req`                                                               | —                                                                         | S1-06, S1-09 |
| S3-18 | `pool.drain <fingerprint>` mgmt verb (cert-rotation aid)                                                                         | —                                                                         | S2-07, S2-15 |

## Bootstrapping problems + solutions

1. **TLS needs cert but ACME is Stage 3** — Stage 2 uses `StaticCertPopulator` only. Tests use `rcgen` to generate self-signed CA + leaf at test runtime (no committed bytes). Production operators bring-your-own cert (`certbot` out-of-band, manual Let's Encrypt, internal CA) until `ManagedCertPopulator` lands.
2. **Crypto provider must be installed before any TLS code** — Stage 1's `main()` already calls `install_default_provider()` even though Stage 1 has no TLS paths. Avoids Stage-2 main() refactor.
3. **Feature-gate rule-compile rejection must land Stage 1** — `FlowGraphMeta.feature_set` + `FlowGraph::link` feature-kind rejection logic lands Stage 1 even though only one feature will switch on then. Stage-2/3 features plug into the existing rejection machinery.
4. **File watcher fires no event for existing files on boot** — boot-time compile is driven explicitly by the boot sequence, not by `notify`. Test in Stage 1 confirms no watcher-fire-on-boot.

## Risk register (to review per stage)

1. **`h3` is still experimental (0.0.x versions).** Stage 3 wiring must keep `h3` behind a feature flag (on by default, documented off-switch) so a critical H3 bug can ship a `vaned` without it.
2. **Wasmtime performance under sustained load untested at S2 commit point.** Land a benchmarks crate alongside S3-04; the 10 ms per-call epoch deadline (see `11-wasm.md`) may need raising for non-trivial plugins.
3. **`pre_exec` requires `unsafe_code` exemption** — CGI module carries `#[allow(unsafe_code)]` with an audit comment documenting the async-signal-safety discipline in the pre_exec closure. Commit names the reviewer.
4. **Reload resets L2 `rate_limit` state.** Correct per spec but an operator surprise magnet. `vane reload --help` surfaces this prominently.
5. **`aws-lc-rs` default requires cmake + C toolchain.** CI must build both `aws-lc-rs` and `ring` feature combinations from Stage 1, not discover breakage at release time.
6. **Sub-agent test-authoring protocol is process discipline, not a tool.** Without commit-metadata enforcement (`Test-Author: sub-agent`), LLMs cut the corner. CI check deferred to post-MVP; until then, reviewer vigilance.
7. **`ring` vs `aws-lc-rs` mutual-exclusion** must be Cargo-level tested — `cargo build --features "ring,aws-lc-rs"` must fail with the `compile_error!`. Otherwise the regression slips past normal `cargo test`.

## Testing matrix (reference — see `spec/testing.md` for policy)

Each feature in this roadmap has a row in the per-stage testing matrix. The sub-agent writing tests for feature `SX-NN` pulls its test-matrix row from the corresponding `spec/architecture/*.md` section that owns the feature, plus the "Anti-over-testing examples" and "Test surface by binary kind" sections of `spec/testing.md`.

Coverage targets per feature align with the `spec/testing.md` 95 % floor — lower only with an in-module documented exemption (I/O error branches with no observable behavior).

## CI ambition (per stage)

CI is **deferred past Stage 1**. This section sketches what CI will check per stage — actual workflow wiring lands when Stage 2 starts. Implementation shape is documented in `spec/architecture/16-crate-layout.md` § _CI orchestration shape_ (shell scripts under `script/`, orchestrated by `Justfile` locally and `.github/workflows/*.yml` in CI).

**Stage 1** — single CI job on `x86_64-unknown-linux-gnu`:

- `just lint` — `cargo clippy --workspace --all-targets -- -D warnings` + `cargo fmt --all -- --check` + `dprint check`
- `just test` — `cargo nextest run --workspace`
- `commitlint` on every push / pre-merge

**Stage 2** — add the target compile-matrix:

- `cargo check` on all Tier 2 targets listed in `spec/architecture/16-crate-layout.md` § _Target tier matrix_, each with its prescribed feature set (32-bit targets drop `wasm`, FreeBSD defaults `wasm` off).
- `just check-mutual-excl` — verifies `cargo build --features "aws-lc-rs,ring"` **fails** with the expected `compile_error!` (per risk-register item 7).
- `just check-no-openssl` — verifies `cargo tree --workspace` contains zero `openssl-sys` (per TLS-library policy; see `spec/architecture/08-tls.md` § _TLS library: rustls only_).
- Full test stays on `x86_64-unknown-linux-gnu`; other Tier 1 targets get `cargo check`.

**Stage 3** — add integration-test jobs:

- WASM-component build: build `vane-wasm-fixture` for `wasm32-wasi`, run `tests/wasm_plugin.rs`.
- ACME: Pebble-backed HTTP-01 integration test via `testcontainers` (slow; gated behind a label or nightly schedule).
- Release-artifact job: emit static binaries for every Tier 1 target (gnu + musl on linux x86_64 / aarch64, plus apple-darwin arm64), `strip`ped, with the expected feature set.

Cross-compile toolchain: `cargo build --target <t>` with rustup targets installed. [`cross`](https://github.com/cross-rs/cross) is reserved for the rare case where the CI runner's glibc is newer than the deploy target's — musl cross-compiles are handled rust-native with a musl C compiler installed on the runner (needed for `aws-lc-rs`'s bindgen step on musl Tier 1 targets).
