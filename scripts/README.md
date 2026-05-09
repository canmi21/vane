# Scripts

Three kinds of file:

- **`*.just`** — flat-imported recipe modules; one topic per file
  (`dev`, `lint`, `doc`, `publish`). Recipe bodies stay one-line —
  anything with control flow, parsing, or HTTP belongs in the
  [`xtask`](../crates/xtask) crate, not in shell-quoted bodies.
- **`bin/vane`, `bin/vaned`** — direnv-loaded thin bash wrappers
  (`cargo build && exec`). Rewriting them in Rust would require
  pre-compiling the wrapper, defeating the purpose.

Workspace logic (publish recipes, `sync-workspace-deps` lefthook
hook, nextest's `build-vane-cli` setup) all routes through
`cargo xtask <subcommand>` (alias in `.cargo/config.toml`).

`scripts/` is not on `PATH` by default — direnv puts `scripts/bin/`
on `PATH` only inside the checkout (see `.envrc`).

## `bin/vane`, `bin/vaned`

Build then `exec`. Two-step (vs. `cargo run`) because cargo's
progress output occasionally leaks through `--quiet` to stderr,
which the TUI's drawing buffer can't survive — building first
pushes those bytes ahead of the terminal takeover.

## xtask reference

`cargo xtask <subcommand>`:

| Subcommand                             | Purpose                                                                                                                             |
| -------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| `build-vane-cli`                       | Build `vane` and write `VANE_BIN=<path>` into `$NEXTEST_ENV` (driven by `.config/nextest.toml`).                                    |
| `sync-deps check` / `sync-deps write`  | Reconcile root `[workspace.dependencies]` `version =` against each crate's own version. `write` preserves formatting via toml_edit. |
| `check-spec-anchors`                   | Verify every `spec/<path>.md § _Section_` reference in source resolves; position-aware with 30-line carry-forward.                  |
| `publish plan [--only X] [--json]`     | Topo-ordered plan; each row is `skip` (on crates.io) or `publish`.                                                                  |
| `publish dry [--only X]`               | `cargo publish --dry-run` per row; verify-build when sibling deps are published, `--no-verify` otherwise.                           |
| `publish run [--only X] [--skip-gate]` | Real publish; runs `just gate` first, polls sparse index between dependents. Requires `CARGO_REGISTRY_TOKEN`.                       |

Drive publish through `just`:

```
just publish-plan                # what would happen
just publish-dry                 # cargo dry-run end-to-end
just publish                     # real publish
just publish-one rustls-pem-roots
just publish --skip-gate         # bypass `just gate`
```

## Setup

direnv hooked into the shell — on macOS + fish:

```fish
brew install direnv
echo 'direnv hook fish | source' >> ~/.config/fish/config.fish
```

Then once per checkout: `cd vane && direnv allow`. From then on,
`cd`-ing into the repo puts `vane` / `vaned` on `PATH`; `cd`-ing
out removes them.
