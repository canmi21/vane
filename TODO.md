# Vane TODO List

**Managed by:** Claude Code (100% AI-managed)
**Last Updated:** 2026-01-02

---

## 🎯 Current Status

**Phase IV (Deep Analysis & Documentation) Complete.**
The codebase has been scanned, documented, and analyzed. New improvement tasks have been identified.

---

## 📋 Roadmap Phase V: Architecture & Quality Improvements

### ✅ Completed Improvements (Phase V)

| ID | Task | Status | Detail |
|----|------|--------|--------|
| 5.1 | **Split `requirements.rs`** | ✅ Done | Extracted `lifecycle.rs` and `watcher.rs`. |
| 5.2 | **Refactor `bootstrap.rs`** | ✅ Done | Modularized into `console.rs`, `logging.rs`, `monitor.rs`. |
| 5.3 | **Flatten `proxy.rs`** | ✅ Done | Reorganized into `transport/proxy/` module. |
| 6.1 | **QUIC Anti-Amplification** | ✅ Done | Implemented global/session byte limits. |
| 6.2 | **TLS Fail-Closed** | ✅ Done | Added strict peek loop and SNI normalization. |
| 6.3 | **Stream Idle Timeouts** | ✅ Done | Added Watchdog with 10s default timeout. |
| 6.4 | **Global L7 Buffer Cap** | ✅ Done | Implemented adaptive memory quota system. |
| 6.5 | **External Env Sanitization** | ✅ Done | Added category-based environment filtering. |

### 🏗️ Code Structure & Organization
*Refining the codebase for maintainability.*

| ID | Task | Priority | Detail |
|----|------|----------|--------|
| 5.4 | **Rename `static.rs`** | Low | Rename `l7/resource/static.rs` to `file_server.rs` to avoid keyword. |
| 5.5 | **Deprecate Legacy Transport** | Low | Mark `modules/stack/transport/legacy` as frozen/deprecated. Plan future removal. |

### ⚡ Performance Optimization
*Reducing overhead in the hot path.*

| ID | Task | Priority | Detail |
|----|------|----------|--------|
| 7.1 | **Optimize KV Hashing** | Low | Switch `KvStore` to `ahash` or `fxhash`. |
| 7.2 | **Lazy Hex Encoding** | ✅ Done | Implemented L4+ hijacking for TLS/QUIC payloads. |
| 7.3 | **Reduce UDP Cloning** | Medium | Use `Bytes` for UDP datagram propagation. |

---

## ✅ Completed Tasks (Phase I - III)

See [CHANGELOG.md](CHANGELOG.md) for history.
- Phase I: Core Architecture (Completed)
- Phase II: Security & Quality (Completed)
- Phase III: Code Organization (Completed)