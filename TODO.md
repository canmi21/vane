# Vane TODO List

**Managed by:** Claude Code (100% AI-managed)

**Last Updated:** 2025-12-30

---

## 🎯 Current Status

**Codebase:** ✅ Good to go (no compiler errors, architecture refined)

**Recent Decisions:**
- ✅ L7 Container design: Protocol extension via `ProtocolData` trait
- ✅ Middleware architecture: Generic (Universal) vs Protocol-Specific (Layered)
- ✅ Template hijacking: Unified engine with layer-specific keywords
- ✅ Roadmap defined: 4 phases (里子 → 查漏 → 面子 → 文档)
- ✅ Plugin error handling: Standardized `failure` branch for external resilience

**Next Step:** Task 0.3 - Architecture Vulnerability Scan

**Detailed Plans:** See `.todo/` directory for full task descriptions

---

## 📋 Overall Roadmap

See [`.todo/roadmap.md`](.todo/roadmap.md) for full details.

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

---

## 🚀 Phase I: Core Architecture Implementation (里子工程)

**Goal:** Implement foundational architecture changes

**Milestone:** Core architecture is generic, extensible, and supports multiple protocols

| ID | Task | Status | File |
|----|------|--------|------|
| 0.3 | Architecture vulnerability scan | 📌 **Next** | [`.todo/architecture-scan.md`](.todo/architecture-scan.md) |
| 1.1 | Rust feature flags support | Pending | [`.todo/rust-feature-flags.md`](.todo/rust-feature-flags.md) |
| 1.4 | Flow validation framework | Pending | [`.todo/flow-validation.md`](.todo/flow-validation.md) |

---

## 🔍 Phase II: Code Quality & Optimization (查漏补缺)

**Goal:** Identify and fix issues in the NEW architecture

**Milestone:** Codebase is secure, optimized, and validated

| ID | Task | Status | File |
|----|------|--------|------|
| 3.x | Flatten stack module hierarchy | Pending | [`.todo/code-organization.md`](.todo/code-organization.md) |
| 3.x | Standardize plugin file structure | Pending | [`.todo/code-organization.md`](.todo/code-organization.md) |

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

**Begin Task 0.3: Architecture Vulnerability Scan**

**Recently Completed (Task 0.2.2):**
- ✅ Refactored plugin system into Generic vs Protocol-Specific tiers.
- ✅ Implemented strict protocol-aware validation (e.g., `httpx` family enforcement).
- ✅ Standardized `failure` branch for all external drivers (HTTP/Unix/Command).
- ✅ Added background connectivity monitor for external middleware.

**Recommended Order**:
```
✅ Task 1.2: Unified Flow Engine
✅ Task 1.3: Hot-Reload Framework
✅ Task 0.2.2: Plugin System Refactoring
→ Task 0.3: Architecture Vulnerability Scan (Next)
```