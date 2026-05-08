# Day-to-day workspace tasks: cargo check / build / test, formatter,
# binary launchers, and the workspace-clean shortcut.

# cargo check across workspace
check:
	cargo check --all-targets --workspace

# cargo build across workspace
build:
	cargo build --all-targets --workspace

# nextest across workspace (default test runner)
test:
	cargo nextest run --workspace

# cargo test bypass — runs doctests; useful when nextest output is suspect
test-cargo:
	cargo test --workspace

# Run a single test by name via nextest expression filter, e.g. `just t1 wss_upstream`
test-one NAME:
	cargo nextest run --workspace -E 'test({{NAME}})'

# Format: rustfmt for .rs, dprint for md/json/toml/yaml (writes changes)
fmt:
	cargo fmt --all
	dprint fmt

# Run vaned (accepts extra args after --)
daemon *args:
	cargo run -p vaned -- {{args}}

# Run vane CLI (accepts extra args after --)
vane *args:
	cargo run -p vane -- {{args}}

# Print --version banner for both binaries
version:
	@cargo run -q -p vane -- --version
	@echo "---"
	@cargo run -q -p vaned -- --version

# Clean build artifacts
clean:
	cargo clean
