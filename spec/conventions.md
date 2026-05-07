# Conventions

## Language

Chat: Simplified Chinese. Code, commits, in-repo docs: English.

## Naming

- **Filenames**: `snake_case.rs` for Rust source. kebab-case for configs, markdown, shell scripts.
- **Identifiers**: `snake_case` for modules / functions / fields; `PascalCase` for types / traits / enums; `SCREAMING_SNAKE_CASE` for constants and statics; short lowercase for lifetimes (`'a`, `'ctx`).
- **Names describe what the thing does, not which vendor provides it.** `handle_error`, not `handle_sentry`. Vendor names appear only in edge modules — the integration boundary where vane meets a dep. Internal logic stays brand-free.

rustfmt and clippy enforce most of this. Match their output.

## Comments

- English, declarative, compact. State the non-obvious _why_ — the constraint, the invariant, the workaround. Never the obvious _what_.
- One or two lines. A block over three lines usually means the function is under-named or over-scoped — fix the code.
- Public items get `///` doc comments. Module-level overviews go in `//!` at the top of the file.
- URLs in comments stay through any compression — they carry context the prose around them does not.
- No separator lines (`// ---`, `// ===`, `/* === */`, ASCII art rules). If a section needs a heading, the function or module boundary already is the heading.
- No commit/PR/issue references in source. Those live in git metadata.

## Testing

`cargo nextest run --workspace` (or `just test`) is the canonical runner. `cargo test --workspace` (`just test-cargo`) is the bypass for doctests and runner-suspect debugging.

- **Local dev target**: `aarch64-apple-darwin`. Cross-target coverage is CI's job.
- **Coverage floor**: 95 % line coverage on tested modules. I/O error branches with no observable behavior may sit below — document the exemption in-module.
- **What to cover**: every public function gets a unit test covering the correct paths plus one representative error path. Exhaustive negative testing is not worth its maintenance cost.
- **Redundancy rule**: if `C` orchestrates `A` and `B`, and `A` / `B` have their own tests, `C`'s tests cover orchestration only. Don't re-verify `A`'s logic through `C`.
- **Anti-over-testing**: don't re-test `PredicateInst::test` inside executor tests; don't re-test hyper's H1 encoding inside terminator tests; don't enumerate the 9-cell HTTP version matrix end-to-end (cover H1↔H1, H2↔H1, H1↔H2, trust hyper for the rest); don't assert hash-cons cache hits in `test()` calls.

### Timing and readiness

`tokio::time::sleep` as a happens-before barrier is flake fuel. Gate on observable state.

- **Push channel** (broadcast, replay-less mpsc) — loop the trigger; each retry hits whatever subscriber state is current.
- **Pull channel** (mgmt verb, file probe) — loop the observation; the producer fires once.
- **Negative-space assertion** (system did _not_ X within `N`) — generous window, polled positive-invariant inside so a regression panics at first divergence.

Short sleeps inside a polling loop (50 ms cadence) are backoff, not barriers.

### Sub-agent test protocol

The agent that writes feature `F` does not write tests for `F`. Tests are written by a sub-agent with spec-only context. The asymmetry catches assumptions the implementation agent would otherwise ratify.

1. Implementation agent commits feature on a branch.
2. Sub-agent receives only the relevant spec sections, public type signatures, and `conventions.md`. It writes failing tests, commits on a sub-branch, does not run them.
3. Implementation agent runs `cargo nextest run`, classifies failures (test bug / impl bug / spec ambiguity), fixes accordingly. Spec-first when both diverge.
4. Merge commit records both authors via `Co-Authored-By` lines.

Pure-derive round-trip tests on `#[derive(Serialize, Deserialize)]` structs with no custom impls are exempt — there is no behavior beyond the derive.

### Test surface by binary kind

- **`vaned`** — full sub-agent automation. Spawn via `vane-testutil::VanedFixture`, drive `curl` / `nc` / `websocat` via `std::process::Command`. Readiness: poll the listener port with `TcpStream::connect_timeout`; never parse stderr (`tracing-subscriber` `fmt` is block-buffered when stderr is not a TTY).
- **`vane` CLI** — full automation. `--json` emits the verb's `result` verbatim; default pretty output auto-disables under `!isatty(stdout)`. Test via `assert_cmd` + `predicates`.
- **`vane` TUI** — partial. Only the pure layer beneath the UI is automated: data adapters (`FlowLogEvent → FlowLogRow`), view state machine (`(state, input) → state`), input mapping. Rendering and crossterm side-effects are verified interactively.

Fixtures live in `vane-testutil`. Add helpers there, never in individual test files.
