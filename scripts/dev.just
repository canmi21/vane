# Day-to-day: check / build / test, formatter, binary launchers, clean.

# Workspace check.
check:
	cargo check --all-targets --workspace

# Workspace build.
build:
	cargo build --all-targets --workspace

# nextest across workspace (default runner).
test:
	cargo nextest run --workspace

# `cargo test` bypass — runs doctests, useful when nextest output is suspect.
test-cargo:
	cargo test --workspace

# Run a single test by name, e.g. `just t1 wss_upstream`.
test-one NAME:
	cargo nextest run --workspace -E 'test({{NAME}})'

# rustfmt for .rs + dprint for md/json/toml/yaml; writes changes.
fmt:
	cargo fmt --all
	dprint fmt

# Run vaned (extra args after --).
daemon *args:
	cargo run -p vaned -- {{args}}

# Run vane CLI (extra args after --).
vane *args:
	cargo run -p vane -- {{args}}

# Print --version banner for both binaries.
version:
	@cargo run -q -p vane -- --version
	@echo "---"
	@cargo run -q -p vaned -- --version

# Clean build artifacts.
clean:
	cargo clean
