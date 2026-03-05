# Contributing Guidelines

This document defines how to contribute to this project.
It exists to protect code quality, maintainability, and maintainers’ time.

---

## 1. General Principles

- Do not submit changes you believe are incorrect.
- Maintainability and long-term readability take priority over speed.
- Maintainers have final authority over all decisions.

---

## 2. Code Quality Requirements

- All code **must pass the language’s default lint or check tools** with no errors.
- All code **must be formatted using the language’s default formatter**.
- Check failure means the contribution will not be reviewed.

---

## 3. Contribution Size & Structure

- Large contributions are welcome, but **must be split into small, focused PRs**.
- Avoid single-source files exceeding **~1000 lines**.
  - This is not a hard limit, but exceeding it requires clear justification.
- Prefer incremental changes over monolithic rewrites.

---

## 4. File Structure & Headers

- Any file considered source code **must start with**:
  1. A comment containing the **relative path from the project root**
  2. A blank line

Example:

```text
// src/module/example.rs
```

---

## 5. Use of LLMs / AI Agents

- LLMs and AI agents may assist implementation.
- They **must remain under your control**.
- AI tools **may not be used to unilaterally decide architecture or design direction**.
- You are responsible for all output they generate.

---

## 6. Workflow Expectations

- Small, obvious fixes may be submitted directly.
- For large ideas or uncertain changes:

- Open an Issue for discussion first.
- If your idea is not accepted:

  > _If you have solid ideas you want to experiment with, make a fork and see how it works._

---

## 7. Reviews & Decision Authority

- All PRs are reviewed by maintainers.
- Acceptance is not guaranteed.
- Silence does not imply approval.
- Maintainers may request changes, restructuring, or rejection without obligation to merge.

---

## 8. Comments & Documentation

- Comments are required where context is non-obvious or decisions matter.
- Do **not** comment every function.
- Comments **must be written in English**.
- Avoid redundant or self-evident comments.

---

## 9. Tests, Changelog, and Releases

- New features **must include tests**:

- Integration tests, mocks, or code-level tests as appropriate.
- Feature PRs must:
  - Modify source files
  - Add an entry to `CHANGELOG.md` under **Unreleased**

- Only maintainers may publish released versions.

---

## 10. Code of Conduct

- All participation must comply with the project’s Code of Conduct.
- Violations may be reported.
- Commit messages **must be clear, objective, and descriptive**, and commits **must be logically grouped and kept compact**.
- **Maintainers have final say on interpretation and enforcement.**

---

By contributing, you agree to follow these rules.
