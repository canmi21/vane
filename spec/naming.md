# Naming

## Filenames

`snake_case.rs` for Rust source files — the module system maps filename ↔ module path, match what rustc expects. kebab-case for configs, markdown, and shell scripts.

## Identifiers

- Modules, functions, variables, fields: `snake_case`.
- Types, traits, enums, type aliases: `PascalCase`.
- Constants and statics: `SCREAMING_SNAKE_CASE`.
- Lifetimes: short, lower-case (`'a`, `'ctx`).

rustfmt and clippy enforce most of this — match their output, don't argue with the tool.

## Objectivity

Names describe **what the thing does**, not **which vendor provides it**. A function that reports errors is `handle_error`, not `handle_sentry`. A module that emits metrics is `metrics`, not `datadog`.

Vendor and brand names may only appear in **edge modules** — the thin integration boundary where the project meets an external dependency.

Internal logic, utility functions, and shared types must stay brand-free. If swapping a library would force renames across the codebase, the abstraction boundary is in the wrong place.
