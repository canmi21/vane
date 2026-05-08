# vane workspace tasks. `just` lists recipes; full names are canonical
# (used in CLAUDE.md / spec) and short aliases (`c`, `b`, `t`, `t1`,
# `g`, `d`, `v`) work everywhere a full name works.

default:
	@just --list --unsorted

# ─── aliases ────────────────────────────────────────────────────────
alias c := check
alias b := build
alias t := test
alias t1 := test-one
alias g := gate
alias d := daemon
alias v := vane

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

# Lint: clippy + rustfmt check + dprint check
lint: lint-clippy lint-fmt lint-prose

# Clippy with -D warnings across workspace
lint-clippy:
	cargo clippy --workspace --all-targets -- -D warnings

# Workspace rustfmt --check
lint-fmt:
	cargo fmt --all -- --check

# dprint --check for prose / config files
lint-prose:
	dprint check

# rustdoc + doctests across the workspace. `RUSTDOCFLAGS=-D warnings`
# turns broken intra-doc links and other rustdoc warnings into errors;
# `cargo test --doc` compiles every `//!` / `///` example and runs the
# ones that are not `no_run`.
doc:
	RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
	cargo test --doc --workspace

# Pre-push gate: full lint pass + workspace test run
gate: lint test

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
