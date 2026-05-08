# scripts/

Repo-local helper scripts and the imported `*.just` recipe modules.
Nothing in this directory is on `PATH` by default — direnv puts
`scripts/bin/` on `PATH` only while the working directory is inside
this checkout (see `.envrc`).

## Layout

- **`*.just`** — recipe modules imported flat by the root `Justfile`,
  one per topic (`dev`, `lint`, `doc`, `publish`).
- **`*.pl`** — perl helpers invoked from those recipes when a task
  needs more than a one-line tool invocation. Perl over POSIX sh /
  bash so the working set stays small and we don't need a Python.
- **`bin/vane`, `bin/vaned`** — direnv-loaded thin wrappers (kept in
  bash because `cargo build && exec` is shorter that way).

## `bin/vane`, `bin/vaned`

Build the corresponding crate in debug mode and `exec` the resulting
binary. Edits under `crates/cli/src/**` or `crates/daemon/src/**`
are picked up by the next invocation; nothing is cached outside
`target/`.

Two-step (build, then exec) rather than `cargo run`: cargo's progress
output leaks through `--quiet` to stderr in some scenarios, and the
TUI's drawing buffer doesn't survive that. Doing the build first
pushes those bytes ahead of the terminal takeover.

## `build-vane-bin.pl`

Builds `crates/cli` (the `vane` binary) and writes
`VANE_BIN=<absolute path>` to the file pointed at by `$NEXTEST_ENV`,
so the daemon's mgmt integration tests can `Command::new` the CLI
without paying a runtime `cargo build` (and the cargo build lock
that would imply across parallel test processes). Run as a nextest
setup script — see `.config/nextest.toml`'s `build-vane-cli` entry.
Path extraction goes through `cargo build --message-format=json`
rather than hard-coding `target/debug/vane`, so the script keeps
working under `CARGO_TARGET_DIR` overrides, `--target <triple>`,
and `--release`.

## `sync-workspace-deps.pl`

Single-source-of-truth keeper for crate versions. Each crate
declares its version in its own Cargo.toml (or inherits from
`[workspace.package].version` for the `vane-*` family); the root
Cargo.toml's `[workspace.dependencies]` `version = "..."` fields
are derived. This script reconciles the two.

Modes:

- `--check` — exit non-zero on drift, list stale entries.
- `--write` — rewrite the root Cargo.toml in place. Used by lefthook
  (with `stage_fixed: true`) so a commit that bumps a lib version
  automatically restages the synced root Cargo.toml.

`publish-execute.pl`'s real mode runs `--check` first and refuses to
proceed on drift, since silently mutating the working tree
mid-publish would be surprising.

## `publish-plan.pl` / `publish-execute.pl`

Workspace-wide publisher to crates.io, split across two scripts so
the plan stage is independently runnable.

`publish-plan.pl` queries the sparse index per crate and emits, in
topological order, the action (`skip` if the version is already on
crates.io, `publish` otherwise), name, version, manifest path, and
intra-workspace deps. Output is auto-detected:

- tty stdout → human-readable table.
- piped stdout → newline-delimited JSON, the contract for
  `publish-execute.pl`.

`publish-execute.pl` reads that JSON on stdin and runs `cargo
publish` per row in `--mode=dry` (verify-build for crates whose
deps are all on crates.io, `--no-verify` otherwise) or
`--mode=real`. Real mode runs `just gate` first (skip with
`--skip-gate`), polls the sparse index between dependents so each
new version is visible before the next verify-build, and aborts on
the first failure. Rerunning skips already-published crates because
the plan re-queries the index every time.

Drive it through `just`:

```
just publish-plan                          # what would happen
just publish-dry                           # cargo dry-run end-to-end
just publish                               # real publish
just publish-one rustls-pem-roots          # single-crate
just publish --skip-gate                   # bypass `just gate`
```

`publish-plan.pl` filters out crates with `publish = false`
(`vane-testutil`, `vane-tests`) via `cargo metadata`. Topological
order comes from non-dev workspace deps; dev-deps don't constrain
order because `cargo publish` strips them.

Real mode requires `CARGO_REGISTRY_TOKEN`. The scripts never
persist credentials — cargo reads the env var directly.

External tools used: `cargo`, `curl`, `just`. Perl `JSON::PP` is
core; no CPAN deps.

## `check_spec_anchors.pl`

Verifies every `spec/<path>.md § _Section_` reference in workspace
source resolves to a real heading in that spec file. Run from the
repo root: `perl scripts/check_spec_anchors.pl`. Exits 0 when
every reference resolves; exits 1 with a grouped report otherwise.

Position-aware: each `§ _Section_` is paired with the closest
preceding `spec/<path>.md` token on the same line, falling back to
the most recent mention in the previous 30 lines so doc-block
headers carry forward to continuation lines. Heading match is exact
(no substring or near-match fallback).

Stdlib-only — `perl` is in the base install on every Unix the
project supports.

## Setup

direnv must be installed and hooked into the shell. On macOS + fish:

```fish
brew install direnv
echo 'direnv hook fish | source' >> ~/.config/fish/config.fish
```

Then, once per checkout:

```
cd vane
direnv allow
```

After that, `cd`-ing into the repo loads `.envrc` automatically and
`vane` / `vaned` resolve to these wrappers. `cd`-ing out unloads it
and the names fall off `PATH`.
