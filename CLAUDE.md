# Vane — Project Rules

> Review and update these rules when project conventions change or no longer apply. Remove outdated rules rather than leaving them as dead weight.

## Communication

- Speak Chinese with the user, keep technical terms in English (e.g. procedure, manifest, codegen)
- All file content (code, comments, docs, commit messages) must be concise declarative English
- No emoji

## Decision Making

- Discuss uncertain matters with the user before proceeding
- Enter plan mode when a single request contains more than 3 tasks
- When self-review reveals potential improvements (performance, design, consistency) that fall outside the current task scope, raise them with the user for discussion rather than silently deferring or silently applying

## Version Control

- Before every `git commit`, run `just fmt && just lint` and fix any errors first; for Rust changes also run `just test-rs`
- For full verification (fmt + lint + build + all tests): `just verify`
- Run `git commit` after each plan mode phase completes, do not push
- Commit messages: conventional commit format (`feat:`, `fix:`, `refactor:`, `docs:`, `test:`, `chore:`, `deps:`, `revert:`, `perf:`); scope is optional and should only be added when it genuinely clarifies context — roughly 1 in 3 commits should have a scope (e.g. `feat(transport):` when the change is transport-specific), the rest use bare prefix (e.g. `refactor: extract shared helpers`)
- Never add AI co-authorship (e.g., "Co-Authored-By: Claude")

## Versioning

- Single source of truth: `Cargo.toml` workspace `version` field; all crates share one version
- Chore-only changes (docs, CI, formatting, tooling) and test-only changes do not bump the version
- Only bump minor for breaking changes: architecture shifts, API incompatibilities, or removed functionality — this **requires explicit user confirmation** before proceeding

## Naming Convention

- Default: lowercase + hyphen (kebab-case) for file names and directory names
- Rust code follows Rust convention: lowercase + underscore (snake_case)
- No uppercase-initial directory or file names unless forced by framework conventions

## Directory Structure

- `src/` uses nested layout organized by functional modules
- Nesting depth must not exceed 4 levels from `src/`
- Use directories to express module boundaries

## Comments

- Write comments, but never state the obvious
- Comments explain why, not what
- During refactoring, do not delete existing comments without first evaluating whether they remain relevant after the refactor

## Code Simplification

- When the user says "简化代码", run the `code-simplifier:code-simplifier` agent to refine the codebase

## Defaults vs Hard-coded Values

- Never hard-code values that users might want to customize (env vars, config params, listen addresses, timeouts, etc.)
- Always provide a sensible default but accept user override via parameter or option
- Rule of thumb: if a user can encounter or configure the value, it must be configurable

## Long-running Tasks

- Use `Bash` with `run_in_background: true` for long-running tasks (builds, full test suites)
- Do not block the main terminal; continue other work while waiting
- Full verification (`just verify`) procedure:
  1. Start in background: `Bash(command: "just verify", run_in_background: true)` — note the returned `task_id`
  2. Poll every 15s: `TaskOutput(task_id, block: false, timeout: 15000)` — compare output with previous poll to detect stalls (no new output for 30s+ = likely stuck)
  3. On completion the system auto-notifies; read final output and report the last 20 lines to the user
- For persistent server processes (dev servers), use tmux: `tmux new-session -d -s <name> '<command>'`

## Refactoring

- File splitting (triggered by length or lint warnings) must be behavior-preserving — no functional changes allowed in the same commit. Typical techniques:
  1. Convert the file into a directory and nest sub-modules inside it
  2. Extract shared logic into a common helper and reuse it across functions
- Rust file split: convert `foo.rs` to `foo/mod.rs` + sub-modules; inner functions become `pub(super)`, only entry-point stays `pub`
- Verify `cargo test --workspace && cargo clippy --workspace` after every Rust structural change

## Agent Team Strategy

- Use Agent Team (TeamCreate) when a plan has 2+ independent sub-tasks that touch different files
- Provide agents with full file contents and exact split instructions; do not rely on agents to read large files themselves
- Always run a unified verification (`cargo test --workspace`) after agents finish before committing
- Shut down agents (SendMessage shutdown_request) once their work is verified
- Discard unrelated formatter diffs (`git checkout -- <file>`) before committing to keep commits focused

## Testing Philosophy

- Pure stateless functions: test correct path + error path (boundary values, empty input, missing keys)
- Composition/orchestration functions: integration-level tests only, do not re-test inner functions
- Go integration tests: test directory under `integration/` — uses `go run main.go` to exercise the vane binary

## Running Tests

| Command                 | Scope                                             |
| ----------------------- | ------------------------------------------------- |
| `just test-rs`          | Rust unit tests (`cargo test --workspace`)        |
| `just test-integration` | Go integration tests (`integration/`)             |
| `just test`             | All tests (Rust + Go integration)                 |
| `just lint`             | All linters (oxlint + clippy + Go + lint-length)  |
| `just lint-check`       | CI check linters (oxlint + Go + links, no build)  |
| `just verify`           | Full pipeline: format + lint + build + all tests  |

## CLI Binary

- Always use the locally compiled CLI from `target/release/vane`, never the system-installed binary
- `cargo install --path src/core` builds and installs it; Rust incremental caching makes no-op rebuilds fast
- `just verify` already handles this automatically
