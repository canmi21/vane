# Vane TODO List

**Managed by:** Claude Code (100% AI-managed)
**Last Updated:** 2026-01-02

---

## 🎯 Current Status

**Phase IV (Deep Analysis & Documentation) Complete.**
The codebase has been scanned, documented, and analyzed. New improvement tasks have been identified.

---

## 📋 Roadmap Phase V: Architecture & Quality Improvements

### 🏗️ Code Structure & Organization
*Refining the codebase for maintainability.*

| ID | Task | Priority | Detail |
|----|------|----------|--------|
| 5.1 | **Split `requirements.rs`** | Low | Extract `watcher.rs` and `lifecycle.rs`. |
| 5.2 | **Refactor `bootstrap.rs`** | Medium | Extract `console.rs` and `logging.rs`. |
| 5.3 | **Flatten `proxy.rs`** | Medium | Split into `transport/proxy/tcp.rs` and `udp.rs`. |
| 5.4 | **Rename `static.rs`** | Low | Rename `l7/resource/static.rs` to `file_server.rs` to avoid keyword. |
| 5.5 | **Deprecate Legacy Transport** | Low | Mark `modules/stack/transport/legacy` as frozen/deprecated. Plan future removal. |

### 🛡️ Security Hardening
*Addressing logic gaps identified in analysis.*

| ID | Task | Priority | Detail |
|----|------|----------|--------|
| 6.1 | **QUIC Anti-Amplification** | High | Enforce strict byte limits on `PENDING_INITIALS` map. |
| 6.2 | **TLS Fail-Closed** | High | Ensure TLS peek failure drops connection instead of passing through. |
| 6.3 | **Stream Idle Timeouts** | Medium | Add `tokio::time::timeout` to all `io::copy` operations in proxies. |
| 6.4 | **Global L7 Buffer Cap** | Medium | Implement global semaphore for `force_buffer` to prevent OOM. |
| 6.5 | **External Env Sanitization** | High | Filter `LD_*` variables in Command driver. |

### ⚡ Performance Optimization
*Reducing overhead in the hot path.*

| ID | Task | Priority | Detail |
|----|------|----------|--------|
| 7.1 | **Optimize KV Hashing** | Low | Switch `KvStore` to `ahash` or `fxhash`. |
| 7.2 | **Lazy Hex Encoding** | Medium | Delay `hex::encode` of TLS/QUIC payloads until accessed in template. |
| 7.3 | **Reduce UDP Cloning** | Medium | Use `Bytes` for UDP datagram propagation. |

---

## ✅ Completed Tasks (Phase I - III)

See [CHANGELOG.md](CHANGELOG.md) for history.
- Phase I: Core Architecture (Completed)
- Phase II: Security & Quality (Completed)
- Phase III: Code Organization (Completed)