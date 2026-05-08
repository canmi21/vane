# crates.io publish pipeline. `publish-plan` is read-only — call it
# any time to see what would happen. `publish-dry` walks the same
# plan through `cargo publish --dry-run`. `publish` is the real
# thing; rerunning after a partial run skips already-published
# crates because the planner re-queries the sparse index.
#
# Real publish runs (in order):
#   1. workspace dep version sync check (must be drift-free)
#   2. just gate                          (skip with --skip-gate)
#   3. cargo xtask publish run
#
# All recipes thin-wrap `cargo xtask publish ...`; see
# `crates/xtask/src/publish.rs` for the actual logic.

# Sync workspace dep `version =` fields to each crate's own version
sync-deps:
	cargo xtask sync-deps write

# Print the publish plan (skip vs publish, in topological order)
publish-plan:
	cargo xtask publish plan

# Dry-run publish: plan + cargo publish --dry-run per crate
publish-dry:
	cargo xtask publish dry

# Real publish (requires CARGO_REGISTRY_TOKEN; pass `--skip-gate` to bypass `just gate`)
publish *args:
	cargo xtask sync-deps check
	cargo xtask publish run {{args}}

# Real publish for a single crate (still topo-checks against the index)
publish-one CRATE *args:
	cargo xtask sync-deps check
	cargo xtask publish run --only={{CRATE}} {{args}}
