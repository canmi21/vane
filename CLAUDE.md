# CLAUDE.md

## Stack

Rust workspace targeting stable. Async runtime: `tokio`. HTTP stack: `hyper` / `h3` / `quinn`. TLS: `rustls`. WASM: `wasmtime`. Concrete crate layout, module boundaries, and trait shapes are documented in [`spec/`](spec/) — start with [`spec/README.md`](spec/README.md).

A Nix flake (`flake.nix`) provides a reproducible dev shell — `nix develop` lands you in an environment with the rust toolchain (read from `rust-toolchain.toml`), `cargo-nextest`, `just`, `bun`, `nixfmt`, and the musl cross-toolchain, all pinned via `flake.lock`. `nix fmt` formats `.nix` files. Optional — system rustup / brew works equivalently.

Generic, project-agnostic crates live under [`crates/lib/`](crates/lib/) and are independently publishable — see [`crates/lib/CLAUDE.md`](crates/lib/CLAUDE.md).

## Quality gates

Everything that can be mechanical is mechanical — treat the gate as authoritative, don't re-check by hand.

Gates run on commit via lefthook:

- `cargo fmt --all -- --check` — formatting (rustfmt config in `rustfmt.toml`)
- `cargo clippy --workspace --all-targets -- -D warnings` — lint
- `commitlint` — conventional commit format, 72-char header, lower-case subject

Test baseline is `cargo nextest run --workspace` (or `just test`); `cargo test --workspace` (or `just test-cargo`) is the bypass for doctests and runner-suspect debugging. Tests are not gated on commit. `just gate` runs lint + test as the pre-push bundle.

When the gate flags formatting or lint issues, **let the toolchain fix what it can mechanically before touching anything by hand**:

- `cargo fmt --all` (without `-- --check`) writes rustfmt's output in place — fixes every formatting issue rustfmt is willing to fix.
- `cargo clippy --workspace --all-targets --fix --allow-dirty --allow-staged` applies clippy's machine-applicable suggestions in place — fixes a substantial fraction of common lint hits.

Run those two before reading the gate output and editing manually. Manually fix only what remains after the auto-pass. Hand-editing whitespace, import ordering, or `format!` argument capture is wasted time when rustfmt + clippy will do it for you.

## Conventions

- **Chat:** Simplified Chinese. **Code / commits / docs-in-repo:** English.
- **Naming, comments, testing:** see [spec/conventions.md](spec/conventions.md).
- When the user says "commit this" without a message, write one that passes commitlint.

## Git

Conventional Commits (see `commitlint.config.js`). Subject ≤ 72 chars, lower-case. No AI co-authorship unless the assistant contributed original design or code beyond following direct instructions.

Prefer the un-scoped `type: subject` form. Use a `(scope)` only when the change touches a non-obvious crate / area and the reader genuinely needs the disambiguation — e.g. `fix(panel): …` when a same-named file exists in multiple crates. Default `feat: …` / `fix: …` / `test: …` over `feat(core): …` etc.; the diff already shows the scope. A rough target: most subjects are scope-less, scopes appear occasionally and only when they earn their keep.

Do **not** reference internal identifiers of any kind in commit subjects or bodies — no stage labels (`Stage 1`, `Stage 2`), no feature IDs (`S1-NN`, `S2-NN`), no chunk identifiers (`C-NN`, `C13.5`), no range shorthand (`S1-28 – S1-30`). Describe what the commit does in plain objective terms: what changed, what it enables. Deferral notes belong in `// TODO(<short-tag>):` source comments or in spec files, not in commit messages.

## Workflow boundaries

- Do **not** propose `/schedule` (or any other Claude Code background-agent invocation) in chunk reports, commit messages, or in-source comments. This project does not use Claude Code's scheduling features. Future-work suggestions belong in `// TODO(<tag>):` source comments or in the relevant `spec/*.md` file.

## Spec index

Each file below is the authoritative source for its topic. Edit there, not here.

- [spec/README.md](spec/README.md) — entry point, principles, reading order, spec ↔ code link rule
- [spec/charter.md](spec/charter.md) — scope, non-goals, design principles
- [spec/conventions.md](spec/conventions.md) — language, naming, comments, testing
- [spec/topology.md](spec/topology.md) — processes, filesystem, daemon and listener lifecycle
- [spec/flow-model.md](spec/flow-model.md) — funnel, FlowGraph IR, compile/link, executor, LazyBuffer, phase machine
- [spec/crates/](spec/crates/) — one file per workspace member (`core`, `engine`, `engine-tls`, `engine-acme`, `engine-wasm`, `mgmt`, `daemon`, `cli`, `clienthello`)
- [spec/wasm-abi.md](spec/wasm-abi.md) — WIT contract for plugin authors
- [spec/tui.md](spec/tui.md) — TUI design (the only outstanding implementation surface)

## Keeping the spec honest

When the toolchain or a project-wide design convention changes, update this file **and** the relevant `spec/*.md` in the **same commit** as the tooling / code change. A rule that contradicts the running code is worse than no rule.

Keep each file tight. If a section starts accumulating step-by-step instructions, war stories, or "don't forget to…" reminders, that is a signal to mechanize it — add a lefthook job, a clippy lint, or a test assertion — and delete the prose. Only project-scope, evergreen rules belong here; everything else either becomes code or gets deleted.
