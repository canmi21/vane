# Vane Project Roadmap (Analysis Phase)

**Status:** 🚧 Deep Codebase Analysis & Re-Documentation in Progress

This phase involves a complete scan of the codebase to generate comprehensive documentation, identify architectural improvements, and plan the next generation of features and fixes.

---

## 🔍 Phase IV: Deep Analysis & Documentation

| ID | Task | Status | Output |
|----|------|--------|--------|
| 4.1 | **Scan Core Infrastructure**<br>(Bootstrap, Router, Socket, Common Utils) | ⏳ Pending | `docs/reference/01-core-infra.md` |
| 4.2 | **Scan L4 Transport Stack**<br>(TCP/UDP Listeners, Proxies, Dispatchers) | ⏳ Pending | `docs/reference/02-l4-transport.md` |
| 4.3 | **Scan L4+ Carrier Stack**<br>(TLS, QUIC, Session Management) | ⏳ Pending | `docs/reference/03-l4p-carrier.md` |
| 4.4 | **Scan L7 Application Stack**<br>(HTTPX, H3, Container, Flow Engine) | ⏳ Pending | `docs/reference/04-l7-application.md` |
| 4.5 | **Scan Plugin System**<br>(Core, Middleware, Terminators, L7 Drivers) | ⏳ Pending | `docs/reference/05-plugin-system.md` |
| 4.6 | **Regenerate Architecture Guide**<br>(High-level system design) | ⏳ Pending | `ARCHITECTURE.md` |
| 4.7 | **Rewrite Developer Guide**<br>(Code navigation, patterns) | ⏳ Pending | `CODE.md` |

---

## 🛠 Phase V: Architecture & Quality Improvements (Planning)

*Specific tasks will be populated in `.todo/` based on Phase IV findings.*

### 📂 Code Organization Proposals
- [ ] Folder structure refinement
- [ ] File naming standardization (Rust keyword avoidance)
- [ ] Dependency injection improvements

### 🛡 Vulnerability & Security
- [ ] Logic gap analysis
- [ ] Resource exhaustion risks
- [ ] Panic safety audit (continuation)

### ⚡ Performance & Reliability
- [ ] Async runtime optimization
- [ ] Memory usage analysis
- [ ] Error propagation refinement
