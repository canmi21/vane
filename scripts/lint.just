# Lint pipeline + the pre-push `gate` (lint + workspace test).

# clippy + rustfmt-check + dprint-check.
lint: lint-clippy lint-fmt lint-prose

# clippy with -D warnings.
lint-clippy:
	cargo clippy --workspace --all-targets -- -D warnings

# rustfmt --check.
lint-fmt:
	cargo fmt --all -- --check

# dprint --check (md/json/toml/yaml).
lint-prose:
	dprint check

# Body invokes via `just` (not prereq deps) so cross-file `test`
# resolves regardless of which justfile was named on entry.

# Pre-push gate: lint + test.
gate:
	just lint
	just test
