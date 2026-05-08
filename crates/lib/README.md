# Library

Generic, project-agnostic Rust crates developed alongside vane
but not bound to its domain. Every crate here is publishable to
crates.io as-is.

## What belongs here

- No `vane-` prefix; named after what the crate does.
- Public API uses generic terminology — no vane-internal names.
- No `vane-*` in `[dependencies]`; tests stay self-contained.
- MIT, matching the workspace.
- Standalone self-managed version.
- MSRV tracks the workspace `rust-toolchain.toml`.
