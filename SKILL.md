# LLM Working Guidelines

This document defines mandatory rules for any LLM working on the codebase.
These rules are binding and non-negotiable.

---

## Project Scope

Vane is a flow-based network protocol engine written in Rust.
It operates across L4, L4+, and L7 using a dynamic, composable pipeline architecture.

Source directory: `src/`

---

## General Authority

- Maintainers have final decision authority.
- You are responsible for all changes you produce.
- Do not perform actions you believe are incorrect.

---

## Language & Style

- All code, comments, and documentation MUST be written in English.
- Do NOT use subjective language.
- Do NOT add time estimates anywhere.

---

## File Header (Mandatory)

Every source file MUST begin with something like this:

```rust
/* src/[relative-path]/[file].rs */
````

Second line MUST be blank. No exceptions.

---

## Comments

- Use comments only where context or decisions matter.
- Do NOT comment every function.
- Public APIs use `///`.
- Implementation details use `//`.

---

## Code Formatting & Quality

- All code MUST pass the language’s default lint or check tools.
- All code MUST be formatted with the language’s default formatter.
- Compilation failure blocks further work.

---

## Contribution Constraints

- Prefer small, focused changes, breaking is allowed but must be discussed.
- Single source files should not exceed ~1000 lines without justification.
- Follow existing architecture and patterns.

---

## Rust Development Workflow (Mandatory)

After ANY code modification:

1. Run `cargo check`
2. Fix all errors
3. Report success
4. WAIT for user instruction

Rules:

- NEVER run `cargo test` without user approval
- NEVER run `cargo build` unless requested
- ALWAYS run `cargo check`

---

## Testing Rules

- Write tests ONLY when explicitly requested.
- Allowed: unit-level Rust tests.
- Allowed: integration or end-to-end tests, but DISALLOWED without instruction.

Integration tests are written in Go under `integration/tests/`.

---

## Hot Reload & Safety

- All config-driven behavior MUST support runtime reload.
- Use atomic update patterns.
- Preserve last-known-good behavior, except update empty file consider as deactivation.

---

## Memory & Performance

- Prefer ownership transfer over cloning.
- Avoid unnecessary allocations.
- Preserve zero-copy paths where possible.

---

## Error Handling

- Use `anyhow::Result`.
- Add context to errors.
- Do not suppress failures.

---

## Documentation

- Architecture documentation lives in `ARCHITECTURE.md`.
- Use factual, objective language.
- Do NOT reference `/examples`.
- Provide complete, independent examples.

---

## Changelog Rules

- Use categories: Breaking, Added, Changed, Fixed (in that order).

---

## Prohibited Actions

- Deviating from file header format
- Mixing languages
- Adding time estimates
- Breaking hot-reload behavior
- Running tests without approval
- Ignoring existing patterns
- Modifying TODO.md without user explicit request

---

## Default Principle

When uncertain:
Follow existing code and documentation and offer suggestions which fit best practice and have clear discussions with users
