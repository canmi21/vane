# crates.io publish, all thin-wrapping `cargo xtask publish ...`
# (logic in crates/xtask/src/publish.rs). Real publish runs:
# sync-deps check → just gate (skippable) → cargo xtask publish run.

# Sync workspace dep `version =` fields to each crate's own version.
sync-deps:
	cargo xtask sync-deps write

# Print the publish plan (skip vs publish, topo-ordered). Read-only.
publish-plan:
	cargo xtask publish plan

# `cargo publish --dry-run` for every plan row.
publish-dry:
	cargo xtask publish dry

# Real publish (requires CARGO_REGISTRY_TOKEN; `--skip-gate` bypasses `just gate`).
publish *args:
	cargo xtask sync-deps check
	cargo xtask publish run {{args}}

# Real publish for a single crate (still topo-checks against the index).
publish-one CRATE *args:
	cargo xtask sync-deps check
	cargo xtask publish run --only={{CRATE}} {{args}}
