# Task 2.22: Dependency Version Unification & Audit

**Goal:** Minimize dependency duplication and prune unnecessary platform-specific crates to reduce binary size and compilation time.

## Status: Investigation Complete

### 1. Key Duplications & Root Causes

| Crate | Versions | Path to Old Version | Path to New Version | Cause |
|-------|----------|---------------------|----------------------|-------|
| `nom` | 7.1, 8.0 | `tls-parser`, `x509-parser` | Vane (direct) | Vane declares `nom 8` but doesn't actually use it. |
| `rustls` | 0.21, 0.23 | `reqwest 0.11` | Vane (direct) | `anynet` -> `ip-lookup` -> `reqwest 0.11`. |
| `reqwest` | 0.11, 0.12 | `anynet` -> `ip-lookup` | Vane (direct) | Legacy version in `anynet` dependency tree. |
| `hyper` | 0.14, 1.8 | `reqwest 0.11` | Vane (direct) | Legacy version in `anynet` dependency tree. |

### 2. Windows-Specific Dependencies
Crates like `windows-sys` and `winapi` are pulled by `tokio` and `socket2`. 
**Verdict:** Acceptable as "ghost dependencies" in `Cargo.lock`. They don't affect UNIX binary size as they are gated by `#[cfg(windows)]`.

---

## Final Implementation Plan

### Step 1: Resolve `nom` Duplication
- **Action:** Remove `nom` from `Cargo.toml`.
- **Reason:** Vane's QUIC parsing is manual; `nom` is unused. This leaves only `nom 7.1` from parser crates.

### Step 2: Resolve `reqwest` / `rustls` Duplication (The Big Cleanup)
- **Problem:** `anynet` pulls in a massive legacy stack (Hyper 0.14, Rustls 0.21, Http 0.2).
- **Action:** 
    1. Implement a lightweight local alternative to `anynet` using the existing `reqwest 0.12`.
    2. Remove `anynet` from `Cargo.toml`.
- **Result:** Removes ~20 transitive legacy dependencies.

### Step 3: Update `deny.toml`
- **Action:** 
    1. Remove `multiple-versions = "deny"` (too strict for transition periods).
    2. Add specific `[[bans.allowed]]` or keep `skip-tree` but focused.
    3. Remove Windows crates from "Deny" since they are required for cross-platform crates to even fetch.

---

## Proposed Local `anynet` Replacement
The current `anynet!` macro seems to:
1. Detect public IP.
2. Log startup info.
We can implement this easily using `reqwest` and `get_if_addrs`.