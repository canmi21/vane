# Testing

`cargo test` is the canonical runner. Unit tests live beside their code in `#[cfg(test)] mod tests` blocks; integration tests live in the workspace-level `tests/` crate.

## Coverage target

95 % line coverage on tested modules. This is a floor, not a ceiling. I/O-heavy code (network, fs) may fall below when the uncovered branches are genuinely error paths with no observable behavior — document the exemption in-module.

## What to cover

Every public function in a leaf crate gets a unit test. Each test module must cover:

- **All correct paths** — every branch that produces a valid result.
- **One error / edge path** — a single representative bad-input case. Exhaustive negative testing is not worth the maintenance cost.

## Redundancy rule

If function C orchestrates functions A and B, and A / B each have their own tests:

- C's tests cover **orchestration logic only** — call order, data threading, short-circuit behavior.
- C's tests do **not** re-verify A's or B's business logic.

Duplication between layers makes refactors painful and signals nothing useful.

## Bug-driven tests (red-green protocol)

When a bug surfaces:

1. **Research first.** Understand root cause before writing anything.
2. **Write a failing test** that captures the exact broken behavior.
3. **Fix the code.** The test goes red → green.
4. Commit test and fix together.

A test written after the fix proves nothing — it is a rubber stamp, not a safety net.

## Test types

| Type        | When to use                                          | Location                         |
| ----------- | ---------------------------------------------------- | -------------------------------- |
| Unit        | Pure functions, zero-dependency logic                | `#[cfg(test)] mod tests` in-file |
| Integration | Cross-crate behavior, public API contracts           | workspace `tests/` crate         |
| Network     | End-to-end traffic against a spawned test server     | `tests/` crate via test-utils    |

Start with unit tests. Introduce integration or network tests when a feature genuinely needs cross-module or transport-level verification — don't pre-build the harness.

## Temporary exemptions

Items listed in `TODO.md` are exempt from coverage requirements. They represent in-flight or placeholder logic that will change imminently. Once the item is resolved and the code stabilizes, tests are required before the next release.
