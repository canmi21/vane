# Vane spec

Vane is a network proxy daemon. The spec describes the architecture; the source is the truth.

## Reading order

1. [`charter.md`](charter.md) — scope, non-goals, design principles.
2. [`conventions.md`](conventions.md) — language, naming, comments, testing.
3. [`topology.md`](topology.md) — processes, filesystem, daemon and listener lifecycle.
4. [`flow-model.md`](flow-model.md) — funnel, FlowGraph IR, compile/link, executor, LazyBuffer, phase machine.
5. [`crates/`](crates/) — one file per workspace member, covering both the top-level crates under `crates/` and the standalone publishable libraries under `crates/lib/`.
6. [`tui.md`](tui.md) — the only outstanding implementation surface.

Plugin authors entry: [`wasm-abi.md`](wasm-abi.md) — the WIT contract `vaned` honors.

## Spec ↔ code

The spec defines architecture (types, contracts, invariants). The source defines implementation. They link both ways:

- A spec section that names a concrete type or function points at the source via a backtick path: `` `crates/engine/src/listener.rs::run_tls` ``.
- A source file implementing a load-bearing contract opens with `//! See spec/<file>.md § _<heading>_`.
- The spec must not duplicate implementation detail; the source must not re-explain architectural reasoning.

When spec and code disagree, the code wins, and the spec is updated in the same commit.

## Status

The MVP is shipped. The TUI in [`tui.md`](tui.md) is the only outstanding green-field surface. Everything else is implemented and covered by `crates/*/tests/` and the workspace `tests/` crate.
