# List available recipes
default:
    @just --list

# Run all tests
test:
    cargo test --workspace

# Run unit tests only (lib tests inside src/)
test-unit:
    cargo test --workspace --lib

# Run integration tests only
test-integration:
    cargo test -p vane-integration-tests

# Run clippy with warnings as errors (all targets including tests)
clippy:
    cargo clippy --workspace --all-targets -- -D warnings

# Format code
fmt:
    cargo fmt --all

# Export panel TypeScript bindings from Rust types
export-types:
    cargo test -p vane-panel --test export_types

# Full CI check: format + clippy + all tests
ci: fmt clippy test
