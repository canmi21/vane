# Vane — unified task runner
# Usage: just <recipe>   |   just --list

set dotenv-load := true
set shell := ["bash", "-euo", "pipefail", "-c"]

# Package manager
pm := "bun"

# List all recipes
default:
    @just --list

# Format + lint (pre-commit gate)
pre-commit: fmt lint

# Run all formatters
fmt:
    chore .
    {{pm}} run fmt:ts
    {{pm}} run fmt:md
    cargo fmt --all
    gofmt -w integration/

# Format TS only (oxfmt)
fmt-ts:
    {{pm}} run fmt:ts

# Format markdown (dprint)
fmt-md:
    {{pm}} run fmt:md

# Format Rust
fmt-rust:
    cargo fmt --all

# Format Go
fmt-go:
    gofmt -w integration/

# Normalize file paths (chore)
fmt-path:
    chore .

# Check formatting without writing
fmt-check:
    {{pm}} run fmt:ts:check
    {{pm}} run fmt:md:check
    cargo fmt --all -- --check
    test -z "$(gofmt -l integration/)"

# Run all linters
lint: lint-ox lint-clippy lint-go lint-length

# Lint TS (oxlint)
lint-ox:
    {{pm}} run lint:ox

# Lint Rust (clippy)
lint-clippy:
    cargo clippy --workspace --all-features --all-targets -- -D warnings -A clippy::unwrap_used -A clippy::print_stdout -A clippy::print_stderr

# Lint Go (golangci-lint)
lint-go:
    cd integration && golangci-lint run ./...

# Warn about files exceeding 500 lines
lint-length:
    bash scripts/lint-length.sh

# Audit all lint-suppression markers (manual)
lint-suppressions:
    bash scripts/lint-suppressions.sh

# Check markdown links
lint-links:
    bash scripts/ci/check-links.sh

# Aggregate lint for CI check job (no build needed)
lint-check: lint-ox lint-go lint-links

# Auto-fix lint issues
lint-fix:
    {{pm}} run lint:ox:fix

# Build Rust workspace
build:
    cargo build --workspace

# Build release
build-release:
    cargo build --release

# Rust unit tests
test-rs:
    cargo test --workspace

# Go integration tests
test-integration:
    #!/usr/bin/env bash
    set -euo pipefail
    export PATH="$HOME/.cargo/bin:$PATH"
    cd integration && go test -v -count=1 -parallel 8 -timeout 120s ./tests/...

# All tests
test: test-rs test-integration

# Full verification pipeline
verify:
    bash scripts/verify-all.sh

# Remove build artifacts
clean:
    cargo clean

# Install vane binary
inst:
    cargo install --path src/core

# Lines of code statistics
scol:
    tokei
