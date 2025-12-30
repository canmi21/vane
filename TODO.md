# Vane TODO List

**Managed by:** Claude Code (100% AI-managed)

**Last Updated:** 2025-12-30

---

## 🎯 Current Status

**Codebase:** ⚠️ **CRITICAL ISSUES FOUND** - 11 critical vulnerabilities discovered in architecture scan

**Recent Decisions:**
- ✅ L7 Container design: Protocol extension via `ProtocolData` trait
- ✅ Middleware architecture: Generic (Universal) vs Protocol-Specific (Layered)
- ✅ Template hijacking: Unified engine with layer-specific keywords
- ✅ Roadmap defined: 4 phases (里子 → 查漏 → 面子 → 文档)
- ✅ Plugin error handling: Standardized `failure` branch for external resilience
- ✅ **Architecture Vulnerability Scan Complete** (2025-12-30): 63 issues identified

**Scan Results:**
- 🔴 CRITICAL: 11 issues (security + reliability + performance)
- 🟠 HIGH: 14 issues
- 🟡 MEDIUM: 16 issues
- 🔵 LOW: 22 issues (mostly 面子 - defer to Phase III)

**Scan Reports:** See `.report/` directory for detailed analysis

**Next Step:** Begin Phase II fixes (Task 2.1 - Task 2.11)

**Detailed Plans:** See `.todo/` directory for full task descriptions

---

## 📋 Overall Roadmap

See [`.todo/roadmap.md`](.todo/roadmap.md) for full details.

### Terminology: 里子 (Core) vs 面子 (Surface)

**里子工程 (Core Work)**: Implementation-level changes that affect code logic, design, and architecture
- Plugin system refactoring, flow engine extraction, protocol abstractions
- Bug fixes, security patches, performance optimizations
- Error handling improvements, validation enhancements
- These changes are about **what the code does** and **how it works internally**

**面子工程 (Surface Work)**: Organization-level changes that affect file structure and naming
- Moving files between directories, renaming modules, flattening hierarchies
- Import path updates, file header corrections
- Code formatting and style consistency
- These changes are about **where the code lives** and **how it looks externally**

**Critical Rule**: Always complete 里子工程 before 面子工程
- Reason 1: Moving files during active refactoring causes merge conflicts and lost work
- Reason 2: After file reorganization, existing documentation (in `docs/`) may become outdated
- Reason 3: Code location changes should be the **last** step before final documentation

### Roadmap Phases

- **Phase I (里子工程)**: Core architecture implementation
- **Phase II (查漏补缺)**: Code quality & optimization on NEW architecture
- **Phase III (面子工程)**: Source folder restructuring
- **Phase IV (文档完善)**: Documentation based on FINAL codebase

---

## ✅ Completed Tasks

| ID | Task | Completed | File |
|----|------|-----------|------|
| 0.1 | Correct Vane positioning in documentation | 2025-12-29 | [`.todo/correct-positioning.md`](.todo/correct-positioning.md) |
| 0.2 | L7 Container design (Generic Container) | 2025-12-29 | [`.todo/container-generalization.md`](.todo/container-generalization.md) |
| 0.2.1 | Container protocol extension (ProtocolData trait) | 2025-12-30 | (inline implementation) |
| 1.5 | Template system upgrade (nested + concatenation) | 2025-12-30 | [`.todo/improve-template.md`](.todo/improve-template.md) |
| 1.2 | Extract unified flow execution engine | 2025-12-30 | [`.todo/extract-flow-engine.md`](.todo/extract-flow-engine.md) |
| 1.3 | Extract hot-reload framework | 2025-12-30 | [`.todo/extract-hotreload.md`](.todo/extract-hotreload.md) |
| 0.2.2 | Plugin system refactoring (Generic vs Specific) | 2025-12-30 | [`.todo/plugin-system-refactor.md`](.todo/plugin-system-refactor.md) |
| 0.3 | Architecture vulnerability scan | 2025-12-30 | [`.todo/architecture-scan.md`](.todo/architecture-scan.md) → [`.report/`](.report/) |
| 2.1 | Management API authentication (SEC-1) | 2025-12-30 | [`.report/security.md#sec-1`](.report/security.md#sec-1) |
| 2.1+ | Integration test optimization (post-auth) | 2025-12-30 | (test framework improvements) |
| 2.7 | Call QUIC session cleanup (memory leak fix) | 2025-12-30 | [`.report/reliability.md#rel-1`](.report/reliability.md#rel-1) |

---

## 🚀 Phase I: Core Architecture Implementation (里子工程)

**Goal:** Implement foundational architecture changes

**Milestone:** Core architecture is generic, extensible, and supports multiple protocols

**Status:** ✅ Phase I Complete - Moving to Phase II (Security & Quality Fixes)

| ID | Task | Status | File |
|----|------|--------|------|
| 1.1 | Rust feature flags support | Deferred | [`.todo/rust-feature-flags.md`](.todo/rust-feature-flags.md) |
| 1.4 | Flow validation framework | Deferred | [`.todo/flow-validation.md`](.todo/flow-validation.md) |

---

## 🔍 Phase II: Security & Quality Fixes (查漏补缺)

**Goal:** Fix critical vulnerabilities and reliability issues discovered in architecture scan

**Milestone:** Codebase is production-ready (secure, reliable, performant)

**Scan Report:** See [`.report/summary.md`](.report/summary.md)

### 🔴 CRITICAL Priority (Must Fix Before Production)

**Security Issues:**

| ID | Task | Severity | Status | Report Reference |
|----|------|----------|--------|------------------|
| 2.1 | Add authentication to management API | 🔴 CRITICAL | ✅ **Done** | [SEC-1](.report/security.md#sec-1) |
| 2.2 | Fix external command injection vulnerability | 🔴 CRITICAL | Pending | [SEC-2](.report/security.md#sec-2) |
| 2.3 | Implement template recursion DoS protection | 🔴 CRITICAL | Pending | [SEC-3](.report/security.md#sec-3) |
| 2.4 | Add template size limits | 🔴 CRITICAL | Pending | [SEC-4](.report/security.md#sec-4) |
| 2.5 | Fix config reload race condition (TOCTOU) | 🔴 CRITICAL | Pending | [SEC-5](.report/security.md#sec-5) |
| 2.6 | Add path canonicalization to loader | 🔴 CRITICAL | Pending | [SEC-6](.report/security.md#sec-6) |

**Reliability Issues:**

| ID | Task | Severity | Status | Report Reference |
|----|------|----------|--------|------------------|
| 2.7 | Call QUIC session cleanup (memory leak fix) | 🔴 CRITICAL | ✅ **Done** | [REL-1](.report/reliability.md#rel-1) |
| 2.8 | Fix QUIC buffer management race condition | 🔴 CRITICAL | Pending | [REL-2](.report/reliability.md#rel-2) |
| 2.9 | Fix external plugin status race | 🔴 CRITICAL | Pending | [REL-3](.report/reliability.md#rel-3) |

**Performance Issues:**

| ID | Task | Severity | Status | Report Reference |
|----|------|----------|--------|------------------|
| 2.10 | Fix flow engine ResolvedInputs cloning | 🔴 CRITICAL | 📌 **Next** | [PERF-1](.report/performance.md#perf-1) |
| 2.11 | Optimize flow path string allocations | 🔴 CRITICAL | Pending | [PERF-2](.report/performance.md#perf-2) |

### 🟠 HIGH Priority (Fix in Next Release)

| ID | Task | Severity | Status | Report Reference |
|----|------|----------|--------|------------------|
| 2.12 | Add template parser complexity protection | 🟠 HIGH | Pending | [SEC-7](.report/security.md#sec-7) |
| 2.13 | Implement template injection protection | 🟠 HIGH | Pending | [SEC-8](.report/security.md#sec-8) |
| 2.14 | Add flow execution timeout | 🟠 HIGH | Pending | [REL-4](.report/reliability.md#rel-4) |
| 2.15 | Replace unwrap() in production code | 🟠 HIGH | Pending | [REL-5](.report/reliability.md#rel-5) |
| 2.16 | Replace unreachable!() with error handling | 🟠 HIGH | Pending | [REL-6](.report/reliability.md#rel-6) |
| 2.17 | Fix rate limiter memory estimation | 🟠 HIGH | Pending | [PERF-3](.report/performance.md#perf-3) |
| 2.18 | Remove unnecessary QUIC frame clones | 🟠 HIGH | Pending | [PERF-4](.report/performance.md#perf-4) |
| 2.19 | Replace blocking I/O with async | 🟠 HIGH | Pending | [PERF-5](.report/performance.md#perf-5) |

### 🟡 MEDIUM Priority (Address Soon)

| ID | Task | Severity | Status | Report Reference |
|----|------|----------|--------|------------------|
| 2.20 | Add logging for QUIC muxer packet drops | 🟡 MEDIUM | Pending | [REL-7](.report/reliability.md#rel-7) |
| 2.21 | Fix Clippy warnings (auto-fixable) | 🟡 MEDIUM | Pending | [.report/maintainability-surface.md](.report/maintainability-surface.md) |
| 2.22 | Resolve nom dependency conflict | 🟡 MEDIUM | Pending | [.report/maintainability-surface.md](.report/maintainability-surface.md) |

**Note:** See detailed reports in `.report/` for complete issue list and remediation steps

---

## 🎨 Phase III: Code Organization (面子工程)

**Goal:** Restructure source folder hierarchy

**Milestone:** Source code organization is clear and logical

| ID | Task | Status | File |
|----|------|--------|------|
| 0.4 | L4 legacy config file extraction | Pending | [`.todo/l4-traditional-config.md`](.todo/l4-traditional-config.md) |
| 3.x | Plugin directory reorganization | Pending | [`.todo/code-organization.md`](.todo/code-organization.md) |

---

## 📚 Phase IV: Documentation (文档完善)

**Goal:** Create comprehensive documentation based on FINAL codebase

**Milestone:** Complete, accurate documentation for users and contributors

- [ ] Update ARCHITECTURE.md with implemented changes
- [ ] Update CODE.md with new folder structure
- [ ] Rewrite docs/overview.md index
- [ ] Create web-based user documentation
- [ ] Create developer documentation
- [ ] Create protocol extension guide

---

## 🔮 Future Work (Priority 2+)

Lower priority tasks deferred until Phase I-III complete.

| Category | File |
|----------|------|
| Performance & Usability | [`.todo/performance-tasks.md`](.todo/performance-tasks.md) |

---

## 📖 Implementation Workflow

**For Each Task:**

1. **Discussion Phase:** Read task file, clarify requirements, agree on approach
2. **Breakdown Phase:** Break into small chunks, define acceptance criteria
3. **Implementation Phase:** Implement one chunk at a time, test after EACH change
4. **Validation Phase:** User reviews, run integration tests, mark complete

**Critical Rule:** Never proceed to implementation before user confirms design.

---

## 🎯 Recommended Next Action

**Begin Phase II: Security & Quality Fixes**

**Recently Completed:**
- ✅ **Task 0.3**: Architecture Vulnerability Scan (2025-12-30)
  - Generated 6 detailed reports in `.report/` directory
  - Identified 63 issues (11 CRITICAL, 14 HIGH, 16 MEDIUM, 22 LOW)
  - Classified issues into 里子 (39 core) vs 面子 (24 surface)
- ✅ **Task 2.1**: Management API Authentication (2025-12-30)
  - Added `ACCESS_TOKEN` environment variable
  - Implemented auth middleware in `src/middleware/auth.rs`
  - Console disabled by default (zero attack surface)
- ✅ **Test Optimization**: Integration test framework improvements (2025-12-30)
  - Replaced fixed waits with log-based port detection
  - Added `WaitForTcpPort()` and `WaitForUdpPort()` helpers
  - Updated 30+ test files, fixed H3/UDP compatibility
  - Test speed: ~21s → ~18s (15% faster)

**Next Steps - CRITICAL Fixes (Task 2.7 - 2.11):**

**This Week (Before Production):**
1. ✅ ~~Task 2.1: Add authentication to management API~~ **DONE**
2. Task 2.7: Call QUIC session cleanup (fix memory leak) ← **NEXT**
3. Task 2.10: Fix flow engine cloning overhead

**Next Week:**
4. Task 2.2: Fix command injection vulnerability
5. Task 2.3: Template DoS protection
6. Task 2.4: Template size limits

**See AGENT.md for detailed fix workflow requirements**

**Recommended Order**:
```
✅ Task 1.2: Unified Flow Engine
✅ Task 1.3: Hot-Reload Framework
✅ Task 0.2.2: Plugin System Refactoring
✅ Task 0.3: Architecture Vulnerability Scan
✅ Task 2.1: Management API Authentication
→ Task 2.7: QUIC Session Cleanup (Next)
```