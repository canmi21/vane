# vaned

Source: [`crates/daemon/`](../../crates/daemon/).

The daemon binary. Glue between `vane-core`, `vane-engine`, `vane-wasm`, and `vane-mgmt`.

## Owns

- `main()` — env loading (`dotenvy`), tracing setup, crypto provider install, config scan, initial compile + link, listener startup, mgmt server mount, signal handlers, watcher spawn. Source: `main.rs`.
- Dependency injection — constructs `Arc<dyn WasmRuntime>` from `vane-wasm`, passes to engine. Constructs `Arc<dyn HttpFetchBackend>` from engine, passes to wasm.
- ACME boot wiring — constructs `ManagedCertRegistry` from `AcmeStore`. Source: `acme_boot.rs`.
- Reload pipeline — file-watcher integration with `ListenerSet::reconcile`, idempotency-via-version-hash. Source: `reload.rs`, `watcher.rs`.
- WASM module loading — content-hash key, `.cwasm` cache, metadata read, registry population. Source: `wasm_loader.rs`.
- mgmt verb handlers — implements `vane_mgmt::Handler` trait against the daemon's owned state. Source: `mgmt_handlers.rs`.
- Provider plumbing — bridges between core's metadata-provider traits and engine's factory registries. Source: `providers.rs`.
- `build.rs` — emits compile-time env vars (commit, build date, rustc version, cargo version) consumed by `main.rs` via `env!()`.

## Crate dependencies

`vane-core`, `vane-engine`, `vane-wasm`, `vane-mgmt` + `tokio`, `dotenvy`, `tracing-subscriber`, `clap` (derive).

## CLI surface

The daemon has exactly two concerns: print build info and start with a config directory.

```
vaned --version  | -v        print build info and exit
vaned --help     | -h        print help and exit
vaned --config DIR | -c DIR  start with the given config directory
```

`--config` is required when starting. `vaned` does not probe for default paths (`/etc/vaned`, `~/.vaned`, …) — it refuses to guess. Running `vaned` with no arguments exits with an error and a hint:

```
error: no config directory specified
hint: `vaned --config /etc/vaned` (or wherever your config lives)
```

This forces explicit config placement in systemd units, Docker images, etc.

## Crypto provider

`vane-engine`'s feature flags pick the rustls crypto provider:

- `aws-lc-rs` (default on Tier 1 targets) — fast (AES-NI / SHA-NI / AVX-512), needs cmake + C compiler at build, FIPS 140-3 optional.
- `ring` — pure Rust + asm, no C toolchain, preferred default on 32-bit Tier 2 targets.

Mutually exclusive at compile time:

```rust
#[cfg(all(feature = "aws-lc-rs", feature = "ring"))]
compile_error!("`aws-lc-rs` and `ring` features are mutually exclusive — pick one");

#[cfg(not(any(feature = "aws-lc-rs", feature = "ring")))]
compile_error!("one of `aws-lc-rs` or `ring` must be enabled");
```

This is orthogonal to the TLS-library policy — both providers are rustls-internal. See [`engine-tls.md` § _Library policy_](engine-tls.md#library-policy).

## Startup sequence

Strict order; any failure in steps 1–6 aborts with non-zero exit and a descriptive stderr message.

1. Parse CLI args via clap. `--config` resolves to a valid directory or process exits with usage error.
2. Load environment variables. OS env wins; then `<config-dir>/.env` is attempted via `dotenvy`. Values in the file fill in variables not already set; they do not overwrite.
3. Install crypto provider — `vane_engine::crypto::install_default_provider()`. Must happen before any TLS code runs.
4. Initialize tracing — `tracing-subscriber`, level from `VANE_LOG_LEVEL` (default `info`), output to stderr (journald captures automatically under systemd).
5. Scan and parse `<config-dir>/config.json` and `<config-dir>/rules/*.json`.
6. Expand / merge / analyze / lower / validate (core) → `Arc<SymbolicFlowGraph>`, then link (engine) → runtime `Arc<FlowGraph>`.
7. Bind listeners. Per-listener bind failures are logged but don't abort boot.
8. Start management transports — Unix socket always (`VANE_MGMT_UNIX`), HTTP-over-TCP default-on at `VANE_MGMT_HTTP_PORT` (3333) and disabled by an explicit empty string.
9. Spawn file watcher on `<config-dir>`, enter run loop.

The watcher is the last setup step. Listeners must be running and the initial `Arc<FlowGraph>` installed before the watcher registers, or a reload event raced ahead of listener bind would have nothing useful to do. If `notify` registration fails (typically permission-denied at the directory level), the daemon logs a warning and continues without auto-reload; reload is then driven by `vane reload` against the management socket, or by daemon restart.

## Boot health watchdog

After listeners start, the daemon polls each listener's bind-ready flag for up to `VANE_BOOT_HEALTH_TIMEOUT_SECS` (default 60 s — covers `VANE_BIND_MAX_ATTEMPTS × VANE_BIND_BACKOFF_MAX_MS`). If zero listeners have bound by the deadline, vaned exits non-zero (no point running with no service). Partial bind (some succeeded, some failed) logs `WARN` and the daemon continues.

## Signals

- **SIGTERM** — drain. Stops accepting on every listener simultaneously, lets in-flight finish up to `VANE_DRAIN_TIMEOUT_SECS` (default 30 s), aborts the rest, exits.
- **SIGHUP** — reload. Same pipeline as the file watcher.
- **SIGINT** — immediate close (developer-friendly).
- **SIGKILL** — bypassed by the kernel. No graceful behavior possible.

## Compiled artifact: in-memory only

`vaned` re-runs the full compile pipeline (`merge → expand → analyze → lower → validate → link`) on every boot and on every reload. The compiled `FlowGraph` exists only in process memory and is never persisted to disk:

- JSON files are the authoritative configuration; the in-memory graph is derived. A persisted compiled artifact would be a third state that can desynchronise.
- Every IR change (new node kind, new field, hash-cons key change) would require explicit cache versioning + invalidation. Forgetting once produces silent miscompiles in production.
- Performance is a non-issue — sub-millisecond pipeline for typical rule counts, dominated by JSON parsing.

Operators who want to inspect compiled state query `get_config` (live), or run `vane compile <DIR>` to dry-run a candidate config tree against the daemon via the `compile_dry_run` mgmt verb (the daemon runs the symbolic pipeline without touching the live graph).

## Tests

`crates/daemon/tests/`:

- `boot.rs` — full daemon spawn, port becomes connectable, mgmt verbs respond, drain on SIGTERM.
- `bind_failure.rs` — bind retry exhaustion, partial-bind tolerance (one family fails).
- `reload.rs` — file-watcher reload, version-hash idempotency, listener-set diff.
- `mgmt.rs`, `mgmt_http.rs`, `mgmt_metrics.rs` — verb-by-verb coverage on Unix and HTTP transports.
- `tls.rs` — TLS termination + listener cert resolution.
- `rate_limit_e2e.rs` — L1 floor + L2 middleware end-to-end.
- `wasm_e2e.rs`, `websocket_e2e.rs` — feature-gated end-to-end paths.

Daemon E2E tests spawn `vaned` as a subprocess (via `assert_cmd` / direct `std::process::Command`) and tear it down at the end of each test; the shared `vane-testutil::VanedFixture` helper is documentation-only today (see `crates/testutil/src/vaned_fixture.rs`).
