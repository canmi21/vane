# Development

## Prerequisites

- [Cargo](https://www.rust-lang.org/tools/install) — Rust build and test
- [Bun](https://bun.sh/) — lint tooling (oxlint, oxfmt, dprint)
- [Go](https://go.dev/) — integration tests
- [just](https://github.com/casey/just) — task runner

## Setup

```bash
bun install
```

## Build

```bash
just build           # cargo build --workspace
just build-release   # cargo build --release
just inst            # cargo install --path src/core
```

## Test

| Command                 | Scope                                            |
| ----------------------- | ------------------------------------------------ |
| `just test-rs`          | Rust unit tests (`cargo test --workspace`)       |
| `just test-integration` | Go integration tests (`integration/`)            |
| `just test`             | All tests (Rust + Go integration)                |
| `just verify`           | Full pipeline: format + lint + build + all tests |

## Format & Lint

```bash
just fmt              # Run all formatters
just lint             # Run all linters
just pre-commit       # Format + lint (pre-commit gate)
just fmt-check        # Check formatting without writing (CI)
just lint-check       # CI aggregate lint (no build needed)
```
