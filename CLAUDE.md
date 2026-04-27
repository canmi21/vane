# CLAUDE.md

## Stack

Rust workspace targeting stable. Async runtime: `tokio`. HTTP stack: `hyper` / `h3` / `quinn`. TLS: `rustls`. WASM: `wasmtime`. Concrete crate layout, module boundaries, and trait shapes are proposed in [`spec/architecture/`](spec/architecture/) — start with [`spec/architecture/README.md`](spec/architecture/README.md).

## Quality gates

Everything that can be mechanical is mechanical — treat the gate as authoritative, don't re-check by hand.

Gates run on commit via lefthook:

- `cargo fmt --all -- --check` — formatting (rustfmt config in `rustfmt.toml`)
- `cargo clippy --workspace --all-targets -- -D warnings` — lint
- `commitlint` — conventional commit format, 72-char header, lower-case subject

Test baseline is `cargo nextest run --workspace` (or `just test`); `cargo test --workspace` (or `just test-cargo`) is the bypass for doctests and runner-suspect debugging. Tests are not gated on commit. `just gate` runs lint + test as the pre-push bundle.

## Conventions

- **Chat:** Simplified Chinese. **Code / commits / docs-in-repo:** English.
- **Naming:** see [spec/naming.md](spec/naming.md).
- **Comments:** see [spec/comments.md](spec/comments.md).
- **Testing:** see [spec/testing.md](spec/testing.md).
- When the user says "commit this" without a message, write one that passes commitlint.

## Git

Conventional Commits (see `commitlint.config.js`). Subject ≤ 72 chars, lower-case. No AI co-authorship unless the assistant contributed original design or code beyond following direct instructions.

Prefer the un-scoped `type: subject` form. Use a `(scope)` only when the change touches a non-obvious crate / area and the reader genuinely needs the disambiguation — e.g. `fix(panel): …` when a same-named file exists in multiple crates. Default `feat: …` / `fix: …` / `test: …` over `feat(core): …` etc.; the diff already shows the scope. A rough target: most subjects are scope-less, scopes appear occasionally and only when they earn their keep.

Do **not** reference internal identifiers of any kind in commit subjects or bodies — no stage labels (`Stage 1`, `Stage 2`), no feature IDs (`S1-NN`, `S2-NN`), no chunk identifiers (`C-NN`, `C13.5`), no range shorthand (`S1-28 – S1-30`). Describe what the commit does in plain objective terms: what changed, what it enables. The link between a commit and the spec feature it implements lives in `spec/roadmap.md` or in the surrounding code's doc comments. Deferral notes belong in `// TODO(<short-tag>):` source comments or in spec files, not in commit messages.

## Workflow boundaries

- Do **not** propose `/schedule` (or any other Claude Code background-agent invocation) in chunk reports, commit messages, or in-source comments. This project does not use Claude Code's scheduling features. Future-work suggestions belong in `// TODO(<tag>):` source comments or in the relevant `spec/*.md` file.

## Spec index

Each file below is the authoritative source for its topic. Edit there, not here.

- [spec/architecture/](spec/architecture/) — system architecture (start with `architecture/README.md`)
- [spec/roadmap.md](spec/roadmap.md) — 3-stage implementation plan, feature IDs (S1-NN / S2-NN / S3-NN), dependency ordering
- [spec/naming.md](spec/naming.md) — identifier and filename conventions
- [spec/comments.md](spec/comments.md) — when and how to write comments
- [spec/testing.md](spec/testing.md) — test structure, coverage, sub-agent testing protocol

## Keeping the spec honest

When the toolchain or a project-wide design convention changes, update this file **and** the relevant `spec/*.md` in the **same commit** as the tooling / code change. A rule that contradicts the running code is worse than no rule.

Keep each file tight. If a section starts accumulating step-by-step instructions, war stories, or "don't forget to…" reminders, that is a signal to mechanize it — add a lefthook job, a clippy lint, or a test assertion — and delete the prose. Only project-scope, evergreen rules belong here; everything else either becomes code or gets deleted.
