# Lint pipeline + the pre-push `gate` that bundles lint with the
# workspace test run. `gate` lives here because the gate concept
# is anchored to the lint pass; the `test` recipe it depends on
# is imported from dev.just.

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

# Pre-push gate: full lint pass + workspace test run
gate: lint test
