# Testing

## Local dev environment

Local development happens on **`aarch64-apple-darwin`** (macOS arm64) exclusively. Every developer — human or LLM — runs `cargo test` against this single platform. Cross-target behaviour is exercised in CI per the Target tier matrix in `spec/architecture/16-crate-layout.md`.

Code that branches on target at runtime must have a macOS arm64 happy path. Target-gated code (`#[cfg(target_os = "linux")]`, `#[cfg(target_family = "unix")]`, etc.) is acceptable, but the crate must still compile on macOS arm64 — use `compile_error!` in a `#[cfg]` branch only if the target is truly unsupported (e.g., Windows).

`cargo test` is the canonical runner. Unit tests live beside their code in `#[cfg(test)] mod tests` blocks; integration tests live in the workspace-level `tests/` crate. End-to-end daemon tests spawn `vaned` as a subprocess via the `vane-testutil::VanedFixture` harness.

See [`spec/architecture/16-crate-layout.md`](architecture/16-crate-layout.md) § _Tests_ for the test-file layout and the [Fixture management](#fixture-management) section below for helper location.

## Coverage target

95 % line coverage on tested modules. This is a floor, not a ceiling. I/O-heavy code (network, fs) may fall below when the uncovered branches are genuinely error paths with no observable behavior — document the exemption in-module.

"95 %" is the line-coverage floor for modules shipping logic. Anti-over-testing: a function C that orchestrates tested functions A and B is covered by testing C's orchestration (call order, short-circuits, data threading) — not by re-testing A's and B's internal branches through C. The 95 % target is satisfied when every C-level branch runs in some test, not when C's tests cover every leaf A and B could reach.

## What to cover

Every public function in a leaf crate gets a unit test. Each test module must cover:

- **All correct paths** — every branch that produces a valid result.
- **One error / edge path** — a single representative bad-input case. Exhaustive negative testing is not worth the maintenance cost.

## Redundancy rule

If function C orchestrates functions A and B, and A / B each have their own tests:

- C's tests cover **orchestration logic only** — call order, data threading, short-circuit behavior.
- C's tests do **not** re-verify A's or B's business logic.

Duplication between layers makes refactors painful and signals nothing useful.

### Anti-over-testing examples

These are concrete patterns to avoid in this project:

- **Re-testing `PredicateInst::test` inside executor tests.** Executor tests verify orchestration (the walk reaches the right node based on match/miss). Do not re-enumerate predicate operators — that belongs to the predicate unit tests.
- **Re-testing hyper's H1 encoding inside `WriteHttpResponse` tests.** Our terminator test asserts "we invoke hyper with a response carrying `Body::Static(Bytes)` plus content-length header"; do not assert the byte-level chunking format. Trust hyper.
- **Re-testing `serde_json` inside config-loader tests.** Our test asserts "a malformed JSON errors with `Compile` kind"; do not re-test serde's rejection of `{...`.
- **Re-testing tokio runtime mechanics.** Assert walker state transitions; do not count `spawn` calls.
- **"Exhaustive combinatorial" tests on the 9-cell HTTP version matrix.** Cover H1↔H1, H2↔H1, H1↔H2 (captures version-translation glue). The other 6 cells are combinations of the same underlying stacks — testing all 9 end-to-end duplicates hyper's own tests.
- **Re-testing hash-consing in `test()` calls.** Hash-cons tests belong to `lower` unit tests; executor tests should not assert "the result is cached" — hash-consing is a memory property, not a call-count one.
- **Re-testing `async_trait` macro expansion** or **`fancy_regex` internals** — rely on the upstream crates' own test suites.

## Sub-agent testing protocol

**Rule:** the LLM that writes code for feature F does not write tests for feature F. Tests are written by a sub-agent whose only inputs are `spec/` and the public type signatures of the code under test.

### Protocol

1. **Main LLM lands the implementation** on a feature branch. References `spec/` throughout.
2. **Main LLM summons a sub-agent** with a fresh context. The sub-agent receives exactly:
   - The `spec/architecture/*.md` files relevant to feature F.
   - `spec/testing.md` (this file).
   - `spec/naming.md`.
   - The public type signatures of F's implementation — **not** the function bodies.
   - The feature's ID and test-matrix row from `spec/roadmap.md`.
3. **Sub-agent writes tests from the spec**, not from the implementation. It lands them in the correct location per the test-location rules in this file. The sub-agent **does not run** the tests. It commits the failing tests on a sub-branch.
4. **Main LLM runs `cargo test`** against its implementation + the sub-agent's tests.
5. **On failure:**
   1. **Do not edit the test first.** A failing test is a signal the implementation is wrong OR the spec is ambiguous.
   2. **Classify the failure**:
      - _Test matches spec, implementation diverges_ → fix the implementation.
      - _Test contradicts spec_ → fix the test (and raise the contradiction for spec clarification).
      - _Neither matches spec_ → fix both, spec-first.
   3. Re-run until green.
6. **On green**, commit the test branch and the implementation branch as a single merge. Commit message records both authors:

   ```
   Co-Authored-By: Claude <sub-agent>     # the test author
   Co-Authored-By: Claude <main>           # the implementation author
   ```

7. **Never:**
   - Main LLM writes tests for its own code in the same session.
   - Sub-agent reads implementation bodies.
   - Sub-agent runs tests.

#### On incidental body exposure

Grep and Read tools return lines of context around a match. A sub-agent searching for `pub fn` signatures may unavoidably see a few lines of adjacent body. This is tool friction, not a protocol breach, and the correct response is:

- **Ignore what was seen.** The sub-agent must still ground each test in `spec/` and the public signatures; it may not write a test that only makes sense given the body it saw.
- **Disclose it.** The sub-agent reports "I incidentally saw impl bodies at `<file:line>` while searching for `<pattern>`" in its commit message or its final report. The human reviewer gains audit signal and can spot-check if the tests drifted toward impl-ratification.
- **Prefer signature-only reads.** When possible, use tools that can return only signatures (e.g., `grep -n '^\s*pub '` + `Read` around that line with a tiny window, rather than reading full files).

A leak that is disclosed and then anchored back to spec is fine. A leak that is not disclosed — and shows up as a test that asserts the impl's accidents rather than the spec's contract — is the real violation.

### Why this protocol

The LLM that made the bug also made assumptions. Tests written under those same assumptions ratify the bug. A sub-agent with spec-only context has no shared assumptions. The asymmetry catches what identical-LLM tests miss.

### Exemption

Pure-derive round-trip tests (e.g., a `serde` round-trip on a `#[derive(Serialize, Deserialize)]` struct with **no custom impls**) may be same-LLM — there is no behavior beyond the derive. All other tests follow the protocol.

### Enforcement

Sub-agent protocol is process discipline, not a tool. Commit metadata makes it auditable:

- Test commits carry `Test-Author: sub-agent` in the trailer.
- A CI rule (post-MVP) fails PRs that commit test and implementation by the same author on the same feature branch.

## Bug-driven tests (red-green sub-agent protocol)

When a bug surfaces:

1. **Research first.** Understand the root cause before writing anything. Document it in the issue or commit message.
2. **Summon a sub-agent** with the spec plus a description of the observed bad behavior — **without** guidance on where the bug lives. Sub-agent writes a failing test that captures the observed behavior.
3. **Main LLM fixes the code.** The sub-agent's test moves red → green.
4. **Commit test and fix together.** The test's author is the sub-agent; the fix's author is the main LLM. Commit message records both.

A test written after the fix, by the fixer, proves nothing — it is a rubber stamp. The sub-agent separation ensures the test captures the spec's intent, not the fixer's assumption about what broke.

## Test types

| Type        | When to use                                                           | Location                                   |
| ----------- | --------------------------------------------------------------------- | ------------------------------------------ |
| Unit        | Pure functions, zero-dependency logic                                 | `#[cfg(test)] mod tests` in-file           |
| Integration | Cross-crate behavior, public API contracts                            | workspace `tests/` crate                   |
| Network     | End-to-end traffic against a spawned test server                      | `tests/` crate via `vane-testutil`         |
| Daemon E2E  | Full `vaned` subprocess lifecycle, `curl` / `nc` / `websocat` traffic | `tests/` via `vane-testutil::VanedFixture` |

Start with unit tests. Introduce integration or network tests when a feature genuinely needs cross-module or transport-level verification — don't pre-build the harness.

## Test surface by binary kind

Testability is asymmetric across the three deliverables:

- **`vaned` (daemon binary)** — full sub-agent automation. The sub-agent can spawn `vaned --config <tmpdir>` as a subprocess, drive `curl` / `wget` / `jq` / `websocat` / `nc` via `std::process::Command`, use `testcontainers` to bring up upstream fixtures (e.g., real nginx as a cross-version H2 upstream). End-to-end tests spin up a real `vaned`, drive traffic, assert on management-API state and flow-log events.

  **Readiness detection — never parse stderr.** `tracing-subscriber`'s default `fmt` subscriber writes block-buffered when stderr is not a TTY; lines like `"listeners started"` may not flush until the process exits, and tests waiting on stderr text deadlock. Poll the listener port with `TcpStream::connect_timeout` in a short loop instead — the connect-success signal mirrors what real clients see, and the lag between "log emitted" and "listener accepting" is the load-bearing one anyway. The same caution applies to log-driven assertions about subsequent lifecycle events: gate on observable state (port closes, file appears, mgmt verb returns), not on log text.
- **`vane` CLI** — full sub-agent automation. `clap`-dispatched subcommands each have two output modes:
  - `--json` — emits the management verb's `result` verbatim (or a defined machine-readable shape for CLI-local commands). Assertable via `jq` piping and `assert_cmd::Command::cargo_bin("vane")`.
  - Default (pretty) — human-friendly tables/trees. Auto-disabled under `!isatty(stdout)`. Test via `assert_cmd` + `predicates` on stdout fragments.
- **`vane` TUI** — **partial** automation only. UI rendering (ratatui widgets, crossterm input) is verified by the user interactively. Sub-agent tests only the pure-function layer beneath the UI:
  - Data adapters: `FlowLogEvent` → `FlowLogRow`, `StatsSnapshot` → `OverviewModel`.
  - View state machine: `(state, input_event) -> new_state` is pure; test with a fixed input trace.
  - Input mapping tables: `KeyEvent` → `Action`.

  No ratatui rendering is tested. No crossterm side-effects are tested. The rendering/input boundary is the LLM/human split.

## Fixture management

`vane-testutil` owns all shared test fixtures. Concrete crate choices:

- **TLS certs**: `rcgen` generates a CA + leaf at test runtime. Fixture bytes are **not** committed to the repo; expiry-flake is impossible. Each test gets a fresh CA scoped to its tmpdir.
- **Free port allocation**: bind `:0`, read the assigned port, re-bind in the system under test. (The test-time race between read and re-bind is accepted; alternative is the `listenfd` crate, deferred.) The race window is widest when many TLS-listener tests run in parallel — handshake setup is heavier than plain TCP, so the per-test interval between port-pick and listener-bind grows. If a TLS-heavy parallel run flakes with `connect: connection refused` or a similar bind-collision symptom, isolate that file with `--test-threads=1` or `cargo test -p <crate> --test <name>` to confirm the flake is collision rather than a real regression. Don't paper over with retries inside the test body.
- **Echo servers**: `vane-testutil::echo_http()`, `echo_tcp()`, `echo_udp()`. Return an `EchoHandle` that auto-teardowns on `Drop`.
- **WASM fixtures**: small `.wasm` components built via `cargo build -p vane-wasm-fixture --target wasm32-wasi --release` in a build script; bytes loaded at test start.
- **CGI fixtures**: small Python scripts shipped under `tests/fixtures/cgi/`; paths handed to `HttpUpstream::Cgi.binary`.
- **Daemon lifecycle**: `VanedFixture` owns tmp config dir + Unix-socket path + child process handle. Waits for socket-ready before returning.
- **ACME directory**: Pebble (`letsencrypt/pebble` Docker image) via `testcontainers` for HTTP-01 live paths; mock DNS server (`hickory-server`) for DNS-01 live paths. See `spec/architecture/08-tls.md` for the ACME test model.

Add fixtures here, **never in individual test files**. Duplication across tests signals a missing testutil helper.

## Temporary exemptions

Items listed in `TODO.md` are exempt from coverage requirements. They represent in-flight or placeholder logic that will change imminently. Once the item is resolved and the code stabilizes, tests are required before the next release.
