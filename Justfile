# vane workspace tasks. `just` lists recipes; full names are
# canonical (used in CLAUDE.md / spec) and short aliases (`c`, `b`,
# `t`, `t1`, `g`, `d`, `v`) work everywhere a full name works.
#
# Recipes are split across scripts/*.just by topic and imported
# below. `import` is flat — every recipe is reachable as
# `just <recipe>`, no namespacing prefix needed. Cross-file
# dependencies (e.g. `gate: lint test`) work because they all
# resolve in the same flat namespace.

import 'scripts/dev.just'
import 'scripts/lint.just'
import 'scripts/doc.just'
import 'scripts/publish.just'

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
