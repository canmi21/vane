# CLAUDE.md

## Stack

Rust workspace targeting stable. Async runtime: `tokio`. HTTP stack: `hyper` / `h3` / `quinn`. TLS: `rustls`. WASM: `wasmtime`. Concrete crate layout, module boundaries, and trait shapes are proposed in [`spec/architecture/`](spec/architecture/) — start with [`spec/architecture/README.md`](spec/architecture/README.md).

## Quality gates

Everything that can be mechanical is mechanical — treat the gate as authoritative, don't re-check by hand.

Gates run on commit via lefthook:

- `cargo fmt --check` — formatting (rustfmt config in `rustfmt.toml`)
- `cargo clippy --workspace --all-targets -- -D warnings` — lint
- `commitlint` — conventional commit format, 72-char header, lower-case subject

## Conventions

- **Chat:** Simplified Chinese. **Code / commits / docs-in-repo:** English.
- **Naming:** see [spec/naming.md](spec/naming.md).
- **Comments:** see [spec/comments.md](spec/comments.md).
- **Testing:** see [spec/testing.md](spec/testing.md).
- When the user says "commit this" without a message, write one that passes commitlint.

## Git

Conventional Commits (see `commitlint.config.js`). Subject ≤ 72 chars, lower-case. No AI co-authorship unless the assistant contributed original design or code beyond following direct instructions.

## Spec index

Each file below is the authoritative source for its topic. Edit there, not here.

- [spec/architecture/](spec/architecture/) — system architecture (start with `architecture/README.md`)
- [spec/naming.md](spec/naming.md) — identifier and filename conventions
- [spec/comments.md](spec/comments.md) — when and how to write comments
- [spec/testing.md](spec/testing.md) — test structure, coverage, red-green protocol

## Keeping the spec honest

When the toolchain or a project-wide design convention changes, update this file **and** the relevant `spec/*.md` in the **same commit** as the tooling / code change. A rule that contradicts the running code is worse than no rule.

Keep each file tight. If a section starts accumulating step-by-step instructions, war stories, or "don't forget to…" reminders, that is a signal to mechanize it — add a lefthook job, a clippy lint, or a test assertion — and delete the prose. Only project-scope, evergreen rules belong here; everything else either becomes code or gets deleted.
