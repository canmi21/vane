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

# Run clippy with warnings as errors
clippy:
    cargo clippy --workspace -- -D warnings

# Full CI check: clippy + all tests
ci: clippy test
