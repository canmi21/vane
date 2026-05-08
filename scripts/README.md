# scripts/

Repo-local helper assets. Three kinds of file live here, each pulling
its weight by being the right tool for what it does:

- **`*.just`** — recipe modules imported flat by the root `Justfile`,
  one per topic (`dev`, `lint`, `doc`, `publish`). Recipes stay one
  line; anything more than `cargo …` / `cargo xtask …` belongs in
  the xtask crate, not in shell-quoted recipe bodies.
- **`bin/vane`, `bin/vaned`** — direnv-loaded thin bash wrappers that
  build the corresponding crate and `exec` the resulting binary. Bash
  is the right tool for `cargo build && exec`; rewriting them in
  Rust would require pre-compiling the wrapper itself, defeating the
  purpose.

Anything that needs control flow, parsing, HTTP, or workspace
introspection lives in the Rust [`xtask`](../crates/xtask) crate —
invoked through the `cargo xtask <subcommand>` alias declared in
`.cargo/config.toml`. The publish recipes in `publish.just` are all
thin wrappers over `cargo xtask publish …`; lefthook's
`sync-workspace-deps` hook calls `cargo xtask sync-deps write`;
nextest's `build-vane-cli` setup script calls `cargo xtask
build-vane-cli`.

This directory is not on `PATH` by default — direnv puts
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

## xtask reference

`cargo xtask <subcommand>` (alias defined in `.cargo/config.toml`):

| Subcommand                             | Purpose                                                                                                                                                                                                                                                                                                     |
| -------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `build-vane-cli`                       | Build the `vane` CLI binary and write `VANE_BIN=<path>` into `$NEXTEST_ENV`. Driven by `.config/nextest.toml`'s `build-vane-cli` setup script.                                                                                                                                                              |
| `sync-deps check` / `sync-deps write`  | Reconcile root Cargo.toml's `[workspace.dependencies]` `version =` fields against each crate's own version. `write` rewrites in place via `toml_edit` (formatting preserved); `check` exits non-zero on drift. lefthook's `sync-workspace-deps` hook calls `write`; the publish recipes call `check` first. |
| `check-spec-anchors`                   | Walk every `spec/<path>.md § _Section_` reference in workspace source comments and verify the heading exists. Position-aware (per-comment-block anchor-to-spec pairing, with carry-forward across the previous 30 source lines).                                                                            |
| `publish plan [--only X] [--json]`     | Print the publish plan in topological order; each row is `skip` (already on crates.io) or `publish`. JSON form is newline-delimited objects.                                                                                                                                                                |
| `publish dry [--only X]`               | `cargo publish --dry-run` per plan row; verify-build for crates whose workspace deps are already on crates.io, `--no-verify` for the rest.                                                                                                                                                                  |
| `publish run [--only X] [--skip-gate]` | Real `cargo publish` per plan row; runs `just gate` first, polls the sparse index between dependents, aborts on the first failure. Requires `CARGO_REGISTRY_TOKEN`.                                                                                                                                         |

Drive the publish workflow through `just` rather than calling xtask
directly:

```
just publish-plan                          # what would happen
just publish-dry                           # cargo dry-run end-to-end
just publish                               # real publish
just publish-one rustls-pem-roots          # single-crate
just publish --skip-gate                   # bypass `just gate`
```

xtask's runtime deps (`cargo`, `just`, network access for the sparse
index, `CARGO_REGISTRY_TOKEN` for real publish) are documented per
subcommand in `crates/xtask/src/*.rs` file comments.

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
