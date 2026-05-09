# Recipes are split across scripts/*.just by topic and imported flat —
# every recipe is reachable as `just <recipe>` (cross-file deps work because
# they all resolve in the same flat namespace).

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
