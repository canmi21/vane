# Contributing Guidelines

This document defines how to contribute to this project.
It exists to protect code quality, maintainability, and maintainers' time.

## General Principles

- Do not submit changes you believe are incorrect.
- Maintainability and long-term readability take priority over speed.
- Maintainers have final authority over all decisions.

## Code Quality Requirements

- All code must pass `just pre-commit` (format + lint) with no errors.
- Check failure means the contribution will not be reviewed.
- Run `just verify` for full verification before submitting.

## Contribution Size & Structure

- Large contributions are welcome, but must be split into small, focused PRs.
- Avoid single-source files exceeding ~500 lines.
  - This is not a hard limit, but exceeding it requires clear justification.
- Prefer incremental changes over monolithic rewrites.

## File Structure & Headers

- Any file considered source code must start with:
  1. A comment containing the relative path from the project root
  2. A blank line

Example:

```text
// src/module/example.rs
```

## Use of LLMs / AI Agents

- LLMs and AI agents may assist implementation.
- They must remain under your control.
- AI tools may not be used to unilaterally decide architecture or design direction.
- You are responsible for all output they generate.

## Workflow Expectations

- Small, obvious fixes may be submitted directly.
- For large ideas or uncertain changes, open an Issue for discussion first.
- If your idea is not accepted:

  > _If you have solid ideas you want to experiment with, make a fork and see how it works._

## Reviews & Decision Authority

- All PRs are reviewed by maintainers.
- Acceptance is not guaranteed.
- Silence does not imply approval.
- Maintainers may request changes, restructuring, or rejection without obligation to merge.

## Comments & Documentation

- Comments are required where context is non-obvious or decisions matter.
- Do not comment every function.
- Comments must be written in English.
- Avoid redundant or self-evident comments.

## Tests, Changelog, and Releases

- New features must include tests: integration tests, mocks, or code-level tests as appropriate.
- Only maintainers may publish released versions.

## Code of Conduct

- All participation must comply with the project's Code of Conduct.
- Commit messages must be clear, objective, and descriptive, and commits must be logically grouped and kept compact.
- Maintainers have final say on interpretation and enforcement.

By contributing, you agree to follow these rules.
