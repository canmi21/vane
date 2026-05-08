# crates.io publish pipeline. `publish-plan` is read-only — call it
# any time to see what would happen. `publish-dry` walks the same
# plan through `cargo publish --dry-run`. `publish` is the real
# thing; rerunning after a partial run skips already-published
# crates because publish-plan.pl re-queries the sparse index.
#
# Real publish runs (in order):
#   1. workspace dep version sync check (must be drift-free)
#   2. just gate                          (skip with --skip-gate)
#   3. publish-plan.pl | publish-execute.pl

# Sync workspace dep `version =` fields to each crate's own version
sync-deps:
	perl scripts/sync-workspace-deps.pl --write

# Print the publish plan (skip vs publish, in topological order)
publish-plan:
	perl scripts/publish-plan.pl

# Dry-run publish: plan + cargo publish --dry-run per crate
publish-dry:
	perl scripts/publish-plan.pl | perl scripts/publish-execute.pl --mode=dry

# Real publish (requires CARGO_REGISTRY_TOKEN; pass `--skip-gate` to bypass `just gate`)
publish *args:
	perl scripts/sync-workspace-deps.pl --check
	perl scripts/publish-plan.pl | perl scripts/publish-execute.pl --mode=real {{args}}

# Real publish for a single crate (still topo-checks against the index)
publish-one CRATE *args:
	perl scripts/sync-workspace-deps.pl --check
	perl scripts/publish-plan.pl --only={{CRATE}} | perl scripts/publish-execute.pl --mode=real {{args}}
