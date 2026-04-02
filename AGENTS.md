# Vane — Project Rules

> Review and update these rules when project conventions change or no longer apply. Remove outdated rules rather than leaving them as dead weight.

## Communication

- Speak Chinese with the user, keep technical terms in English (e.g. proxy, transport, upstream)
- All file content (code, comments, docs, commit messages) must be concise declarative English
- No emoji

## Decision Making

- Discuss uncertain matters with the user before proceeding
- Enter plan mode when a single request contains more than 3 tasks
- When self-review reveals potential improvements (performance, design, consistency) that fall outside the current task scope, raise them with the user for discussion rather than silently deferring or silently applying

## Version Control

- Before every `git commit`, run `just ci` (fmt + clippy + test) and fix any errors first
- Run `git commit` after each plan mode phase completes, do not push
- Commit messages: conventional commit format (`feat:`, `fix:`, `refactor:`, `docs:`, `test:`, `chore:`, `deps:`, `revert:`, `perf:`); scope is optional and should only be added when it genuinely clarifies context — roughly 1 in 3 commits should have a scope (e.g. `feat(transport):` when the change is transport-specific), the rest use bare prefix (e.g. `refactor: extract shared helpers`)
- Commit messages must not mention version bumps unless the version was actually changed

## Versioning

- Single source of truth: `Cargo.toml` workspace `version` field; all crates inherit via `version.workspace = true`
- Chore-only changes (docs, CI, formatting, tooling) and test-only changes do not bump the version
- Only bump minor for breaking changes: architecture shifts, API incompatibilities, or removed functionality — this **requires explicit user confirmation** before proceeding

## Naming Convention

- Default: lowercase + hyphen (kebab-case) for file names and directory names
- Rust code follows Rust convention: lowercase + underscore (snake_case)
- No uppercase-initial directory or file names unless forced by framework conventions

## Monorepo Structure

Rust workspace with `resolver = "3"` and `edition = "2024"`:

| Crate                    | Path             | Role                                                                                               |
| ------------------------ | ---------------- | -------------------------------------------------------------------------------------------------- |
| `vane-primitives`        | `src/primitives` | Shared types and foundational utilities                                                            |
| `vane-transport`         | `src/transport`  | TCP listener, bidirectional proxy, transport errors                                                |
| `vane-engine`            | `src/engine`     | Route table, connection dispatch, engine lifecycle                                                 |
| `vane-extra`             | `src/extra`      | Extended functionality                                                                             |
| `vane`                   | `src/vane`       | Binary entry point                                                                                 |
| `vane-test-utils`        | `src/test-utils` | Test helpers (echo server, mock server, tracing init, timeout assertions); dev-dependency only     |
| `vane-integration-tests` | `tests`          | Workspace-level integration tests, organized by domain (`tests/engine/`, `tests/transport/`, etc.) |

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

## Running Tests

| Command                 | Scope                                                           |
| ----------------------- | --------------------------------------------------------------- |
| `just test`             | All tests (`cargo test --workspace`)                            |
| `just test-unit`        | Unit tests only (`cargo test --workspace --lib`)                |
| `just test-integration` | Integration tests only (`cargo test -p vane-integration-tests`) |
| `just clippy`           | Clippy with `-D warnings`                                       |
| `just fmt`              | Format code (`cargo fmt --all`)                                 |
| `just ci`               | Full CI check: fmt + clippy + all tests                         |

## Testing Philosophy

- Pure stateless functions: test correct path + error path (boundary values, empty input, missing keys)
- Composition/orchestration functions: integration-level tests only, do not re-test inner functions
- Use `vane-test-utils` helpers to avoid boilerplate: `EchoServer::start()`, `MockTcpServer::start(handler)`, `init_tracing()`, `assert_within()` / `assert_timeout()`

## Agent Team Strategy

- Use Agent Team (TeamCreate) when a plan has 2+ independent sub-tasks that touch different files
- Agents create their own sub-tasks; lead monitors via TaskList
- Provide agents with full file contents and exact split instructions; do not rely on agents to read large files themselves
- Always run unified verification (`just ci`) after agents finish before committing
- Shut down agents (SendMessage shutdown_request) once their work is verified

## Refactoring

- File splitting (triggered by length or lint warnings) must be behavior-preserving — no functional changes allowed in the same commit
- Rust file split: convert `foo.rs` to `foo/mod.rs` + sub-modules; inner functions become `pub(super)`, only entry-point stays `pub`
- Verify `just ci` after every Rust structural change
