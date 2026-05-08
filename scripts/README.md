# scripts/

Repo-local helper scripts. Not on `PATH` by default — direnv puts
`scripts/bin/` on `PATH` only while the working directory is inside
this checkout (see `.envrc`).

## `bin/vane`, `bin/vaned`

Build the corresponding crate in debug mode and `exec` the resulting
binary. Edits under `crates/cli/src/**` or `crates/daemon/src/**`
are picked up by the next invocation; nothing is cached outside
`target/`.

Two-step (build, then exec) rather than `cargo run`: cargo's progress
output leaks through `--quiet` to stderr in some scenarios, and the
TUI's drawing buffer doesn't survive that. Doing the build first
pushes those bytes ahead of the terminal takeover.

## `build-vane-bin.sh`

Builds `crates/cli` (the `vane` binary) and writes
`VANE_BIN=<absolute path>` to the file pointed at by `$NEXTEST_ENV`,
so the daemon's mgmt integration tests can `Command::new` the CLI
without paying a runtime `cargo build` (and the cargo build lock
that would imply across parallel test processes). Run as a nextest
setup script — see `.config/nextest.toml`'s `build-vane-cli`
entry. Path extraction goes through `cargo build
--message-format=json` rather than hard-coding `target/debug/vane`,
so the script keeps working under `CARGO_TARGET_DIR` overrides,
`--target <triple>`, and `--release`.

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
cd <repo>
direnv allow
```

After that, `cd`-ing into the repo loads `.envrc` automatically and
`vane` / `vaned` resolve to these wrappers. `cd`-ing out unloads it
and the names fall off `PATH`.
