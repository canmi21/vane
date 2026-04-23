# vane workspace tasks. Run `just` to list recipes.

default:
	@just --list --unsorted

# cargo check across workspace
c:
	cargo check --all-targets --workspace

# cargo build across workspace
b:
	cargo build --all-targets --workspace

# cargo test across workspace
t:
	cargo test --workspace

# Format: rustfmt for .rs, dprint for md/json/toml/yaml
fmt:
	cargo fmt --all
	dprint fmt

# Lint: clippy + rustfmt check + dprint check
lint:
	cargo clippy --workspace --all-targets -- -D warnings
	cargo fmt --all -- --check
	dprint check

# Run vaned (accepts extra args after --)
d *args:
	cargo run -p vaned -- {{args}}

# Run vane CLI (accepts extra args after --)
v *args:
	cargo run -p vane -- {{args}}

# Print --version banner for both binaries
version:
	@cargo run -q -p vane -- --version
	@echo "---"
	@cargo run -q -p vaned -- --version

# Clean build artifacts
clean:
	cargo clean
