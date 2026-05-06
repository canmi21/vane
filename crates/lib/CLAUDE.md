# crates/lib/

Generic, project-agnostic Rust crates that are developed alongside vane
but **not bound to vane's domain or names**. Every crate under this
directory is publishable to crates.io as-is.

Conventions:

- **Crate name has no `vane-` prefix**. Use the descriptive function-
  level name (e.g. `clienthello`, not `vane-quic-peek`).
- **Public API names are generic**. No `vane`, `flow_graph`, `executor`,
  or any other vane-internal terminology in public types or fns. The
  API surface is shaped by what the crate actually does, named after
  the operation (`extract_sni`, `parse_initial_header`).
- **No path-level vane dependency**. `Cargo.toml` may not list any
  `vane-*` crate under `[dependencies]`. Tests inside the crate may
  not reach into `vane-*` either.
- **Best-effort generality**. The public API is shaped to fit vane's
  needs first; we don't pay extra cost to support hypothetical use
  cases. But within that constraint, prefer the more general option
  when it costs nothing extra (e.g. taking `&[u8]` over `&Bytes`,
  returning `Result` over panic).
- **License**: same MIT as the workspace; each crate's `Cargo.toml`
  carries `license = "MIT"` explicitly so it survives a checkout from
  crates.io without the workspace `Cargo.toml` context.
- **MSRV**: same as the workspace `rust-version`. Crates here track
  the workspace's `rust-toolchain.toml`.

Crates currently in this region:

- `clienthello/` — extract the TLS SNI from a QUIC client's Initial
  datagrams without performing a full handshake.
