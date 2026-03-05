# Claude Agent Guidelines

**NOTICE:** Project-level coding standards, workflows, and conventions live in [SKILL.md](SKILL.md).
This file governs Claude-specific behavior: communication, decision-making, tooling, and collaboration patterns.

---

## Communication

- Speak Chinese with the user; keep technical terms in English (e.g. procedure, manifest, codegen)
- All file content (code, comments, docs, commit messages) must be concise declarative English
- No emoji

## Decision Making

- Discuss uncertain matters with the user before proceeding
- Enter plan mode when a single request contains more than 3 tasks
- When self-review reveals potential improvements (performance, design, consistency) outside the current task scope, raise them with the user rather than silently deferring or silently applying

## Version Control

- Before every `git commit`, run formatting and lint checks and fix any errors first; for Rust changes also run `cargo check`
- Run `git commit` after each plan mode phase completes; do not push
- Commit messages: conventional commit format (`feat:`, `fix:`, `refactor:`, `docs:`, `test:`, `chore:`, `deps:`, `revert:`, `perf:`); scope is optional and should only be added when it genuinely clarifies context
- Never add AI co-authorship (e.g. "Co-Authored-By: Claude")

## Comments

- Write comments, but never state the obvious
- Comments explain why, not what
- During refactoring, do not delete existing comments without first evaluating whether they remain relevant

## Naming Convention

- Default: lowercase + hyphen (kebab-case) for file names and directory names
- Rust code follows Rust convention: lowercase + underscore (snake_case)
- No uppercase-initial directory or file names unless forced by framework conventions

## Directory Structure

- `src/` uses nested layout organized by functional modules
- Nesting depth must not exceed 4 levels from `src/`
- Use directories to express module boundaries

## Defaults vs Hard-coded Values

- Never hard-code values that users might want to customize
- Always provide a sensible default but accept user override via parameter or option
- Rule of thumb: if a user can encounter or configure the value, it must be configurable

## Long-running Tasks

- Use `Bash` with `run_in_background: true` for long-running tasks (builds, full test suites)
- Do not block the main terminal; continue other work while waiting
- For persistent server processes, use tmux: `tmux new-session -d -s <name> '<command>'`

## Refactoring

- Rust file split: convert `foo.rs` to `foo/mod.rs` + sub-modules; inner functions become `pub(super)`, only entry-point stays `pub`
- Verify `cargo check` (and `cargo test --workspace` when approved) after every Rust structural change

## Agent Team Strategy

- Use Agent Team (TeamCreate) when a plan has 2+ independent sub-tasks that touch different files
- Provide agents with full file contents and exact split instructions; do not rely on agents to read large files themselves
- Always run unified verification after agents finish before committing
- Shut down agents once their work is verified
- Discard unrelated formatter diffs before committing to keep commits focused

## Testing Philosophy

- Pure stateless functions: test correct path + error path (boundary values, empty input, missing keys)
- Composition/orchestration functions: integration-level tests only, do not re-test inner functions

## Code Simplification

- When the user says "简化代码", run the `code-simplifier:code-simplifier` agent to refine the codebase
