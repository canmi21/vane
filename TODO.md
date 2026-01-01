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
| 2.1 | Management API authentication | 2025-12-30 | [Security Report](.report/security.md) |
| 2.1+ | Integration test optimization (post-auth) | 2025-12-30 | (test framework improvements) |
| 2.7 | Call QUIC session cleanup (memory leak fix) | 2025-12-30 | [Reliability Report](.report/reliability.md) |
| 2.10 | Fix flow engine ResolvedInputs cloning | 2025-12-30 | [Performance Report](.report/performance.md) |
| 2.11 | Optimize flow path string allocations | 2025-12-30 | [Performance Report](.report/performance.md) |
| 2.2 | Fix external command injection vulnerability | 2025-12-30 | [Security Report](.report/security.md) |
| 2.3 | Implement template recursion DoS protection | 2025-12-30 | [Security Report](.report/security.md) |
| 2.4 | Add template size limits | 2025-12-30 | [Security Report](.report/security.md) |
| 2.5 | Fix config reload race condition (TOCTOU) | 2025-12-30 | [Security Report](.report/security.md) |
| 2.6 | Add path canonicalization to loader | 2025-12-30 | [Security Report](.report/security.md) |
| 2.8 | Fix QUIC buffer management race condition | 2025-12-30 | [Reliability Report](.report/reliability.md) |
| 2.9 | Fix external plugin status race | 2025-12-30 | [Reliability Report](.report/reliability.md) |
| 2.12 | Add template parser complexity protection | 2025-12-30 | [Security Report](.report/security.md) |
| 2.13 | Implement template injection protection | 2025-12-30 | [Security Report](.report/security.md) |
| 2.14 | Add flow execution timeout | 2025-12-30 | [Reliability Report](.report/reliability.md) |
| 2.16 | Replace unreachable!() with error handling | 2025-12-30 | [Reliability Report](.report/reliability.md) |
| 2.17 | Fix rate limiter memory estimation | 2025-12-30 | [Performance Report](.report/performance.md) |
| 2.18 | Remove unnecessary QUIC frame clones | 2025-12-30 | [Performance Report](.report/performance.md) |
| 2.19 | Replace blocking I/O with async | 2025-12-30 | [Performance Report](.report/performance.md) |

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

| ID | Task | Status | File |
|----|------|--------|------|
| 2.1 | Add authentication to management API | 🔴 CRITICAL | ✅ **Done** |
| 2.2 | Fix external command injection vulnerability | 🔴 CRITICAL | ✅ **Done** |
| 2.3 | Implement template recursion DoS protection | 🔴 CRITICAL | ✅ **Done** |
| 2.4 | Add template size limits | 🔴 CRITICAL | ✅ **Done** |
| 2.5 | Fix config reload race condition (TOCTOU) | 🔴 CRITICAL | ✅ **Done** |
| 2.6 | Add path canonicalization to loader | 🔴 CRITICAL | ✅ **Done** |

**Reliability Issues:**

| ID | Task | Severity | Status | Report Reference |
|----|------|----------|--------|------------------|
| 2.7 | Call QUIC session cleanup (memory leak fix) | 🔴 CRITICAL | ✅ **Done** | [Reliability Report](.report/reliability.md) |
| 2.8 | Fix QUIC buffer management race condition | 🔴 CRITICAL | ✅ **Done** | [Reliability Report](.report/reliability.md) |
| 2.9 | Fix external plugin status race | 🔴 CRITICAL | ✅ **Done** | [Reliability Report](.report/reliability.md) |

**Performance Issues:**

| ID | Task | Severity | Status | Report Reference |
|----|------|----------|--------|------------------|
| 2.10 | Fix flow engine ResolvedInputs cloning | 🔴 CRITICAL | ✅ **Done** | [Performance Report](.report/performance.md) |
| 2.11 | Optimize flow path string allocations | 🔴 CRITICAL | ✅ **Done** | [Performance Report](.report/performance.md) |

### 🟠 HIGH Priority (Fix in Next Release)

| ID | Task | Severity | Status | Report Reference |
|----|------|----------|--------|------------------|
| 2.12 | Add template parser complexity protection | 🟠 HIGH | ✅ **Done** | [Security Report](.report/security.md) |
| 2.13 | Implement template injection protection | 🟠 HIGH | ✅ **Done** | [Security Report](.report/security.md) |
| 2.14 | Add flow execution timeout | 🟠 HIGH | ✅ **Done** | [Reliability Report](.report/reliability.md) |
| 2.15 | Replace unwrap() in production code | 🟠 HIGH | ✅ **Done** | [Panic Safety List](.todo/replace-unwrap.md) |
| 2.16 | Replace unreachable!() with error handling | 🟠 HIGH | ✅ **Done** | [Reliability Report](.report/reliability.md) |
| 2.17 | Fix rate limiter memory estimation | 🟠 HIGH | ✅ **Done** | [Performance Report](.report/performance.md) |
| 2.18 | Remove unnecessary QUIC frame clones | 🟠 HIGH | ✅ **Done** | [Performance Report](.report/performance.md) |
| 2.19 | Replace blocking I/O with async | 🟠 HIGH | ✅ **Done** | [Performance Report](.report/performance.md) |
| 2.20 | Implement L4/L4+ connection rate limits | 🟠 HIGH | Pending | [Security Report](.report/security.md) |

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
- ✅ **Startup Optimization**: Eliminated 4.4s bootstrap delay.
- ✅ **Management API Authentication**: Added mandatory token auth.
- ✅ **External Command Security**: Implemented Trusted Bin Root policy.
- ✅ **Template Security**: Added recursion depth, result size limits, AST complexity protection, and injection protection.
- ✅ **Config Reliability**: Fixed reload race conditions and implemented Keep-Last-Known-Good strategy.
- ✅ **Path Security**: Implemented mandatory path canonicalization in configuration loader.
- ✅ **QUIC Reliability**: Fixed buffer race conditions and enforced packet limits.
- ✅ **Resource Management**: Implemented precise memory tracking for rate limiters.
- ✅ **Performance Optimization**: Eliminated redundant clones and completed full Async I/O migration.

**Next Steps - Fixes:**

1. Task 2.15: Continue replacing unwrap() in production code ← **NEXT**
2. Task 2.20: Implement L4/L4+ connection rate limits

**See AGENT.md for detailed fix workflow requirements**

**Recommended Order**:
```
✅ Task 1.2: Unified Flow Engine
✅ Task 1.3: Hot-Reload Framework
✅ Task 0.2.2: Plugin System Refactoring
✅ Task 0.3: Architecture Vulnerability Scan
✅ Task 2.1: Management API Authentication
✅ Task 2.7: QUIC Session Cleanup
✅ Task 2.10: Flow Engine Cloning Fix
✅ Task 2.11: Flow Path Optimization
✅ Task 2.2: Command Injection Fix
✅ Task 2.3: Template DoS Protection
✅ Task 2.4: Template Size Limits
✅ Task 2.5: Config Reload Race Fix
✅ Task 2.6: Path Canonicalization
✅ Task 2.8: QUIC Buffer Race Fix
✅ Task 2.9: External Plugin Status Fix
✅ Task 2.12: Template Complexity Protection
✅ Task 2.13: Template Injection Protection
✅ Task 2.14: Flow Execution Timeout
✅ Task 2.16: Elimination of Panics
✅ Task 2.17: Precise Rate Limit Tracking
✅ Task 2.18: QUIC Frame Optimization
✅ Task 2.19: Full Async I/O Migration
→ Task 2.15: Continue Panic Safety (Next)
```
