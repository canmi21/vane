# Vane TODO List

**Managed by:** Claude Code (100% AI-managed)

**Last Updated:** 2025-12-29

---

## 🎯 Current Status

**Codebase:** ✅ Good to go (no compiler errors, ready for refactoring)

**Recent Decisions:**
- ✅ L7 Container design: Generic `Container<P: ProtocolData>`
- ✅ Middleware architecture: 通用 (KV-based) vs 细分 (protocol-specific)
- ✅ Template hijacking: Layer-specific keywords, no KV pollution
- ✅ Roadmap defined: 4 phases (里子 → 查漏 → 面子 → 文档)

**Next Step:** Start Phase I with Task 1.5 (Template system upgrade)

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

---

## 🚀 Phase I: Core Architecture Implementation (里子工程)

**Goal:** Implement foundational architecture changes

**Milestone:** Core architecture is generic, extensible, and supports multiple protocols

| ID | Task | Status | Estimate | File |
|----|------|--------|----------|------|
| 1.5 | Template system upgrade (nested + concatenation) | 📌 **Next** | 3-5 days | [`.todo/improve-template.md`](.todo/improve-template.md) |
| 0.2 | L7 Container implementation (5 phases) | Pending | 10-15 days | [`.todo/container-generalization.md`](.todo/container-generalization.md) |
| 1.2 | Extract flow execution engine | Pending | 3-4 days | [`.todo/extract-flow-engine.md`](.todo/extract-flow-engine.md) |
| 1.3 | Extract hot-reload framework | Pending | 2-3 days | [`.todo/extract-hotreload.md`](.todo/extract-hotreload.md) |

---

## 🔍 Phase II: Code Quality & Optimization (查漏补缺)

**Goal:** Identify and fix issues in the NEW architecture

**Milestone:** Codebase is secure, optimized, and validated

| ID | Task | Status | File |
|----|------|--------|------|
| 0.3 | Architecture vulnerability scan | Pending | [`.todo/architecture-scan.md`](.todo/architecture-scan.md) |
| 0.4 | L4 traditional configuration strategy | Pending | [`.todo/l4-traditional-config.md`](.todo/l4-traditional-config.md) |
| 1.1 | Rust feature flags support | Pending | [`.todo/rust-feature-flags.md`](.todo/rust-feature-flags.md) |
| 1.4 | Flow validation framework | Pending | [`.todo/flow-validation.md`](.todo/flow-validation.md) |

---

## 🎨 Phase III: Code Organization (面子工程)

**Goal:** Restructure source folder hierarchy

**Milestone:** Source code organization is clear and logical

| ID | Task | Status | File |
|----|------|--------|------|
| 3.x | Plugin directory reorganization | Pending | [`.todo/code-organization.md`](.todo/code-organization.md) |
| 3.x | Flatten stack module hierarchy | Pending | [`.todo/code-organization.md`](.todo/code-organization.md) |
| 3.x | Standardize plugin file structure | Pending | [`.todo/code-organization.md`](.todo/code-organization.md) |

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

**Start Phase I with Task 1.5: Template System Upgrade**

**Why this task first:**
- Foundation for template hijacking mechanism (needed for Task 0.2)
- Relatively independent, doesn't affect other modules
- Can be tested immediately after completion

**Next steps:**
1. Analyze current template system code
2. Design AST structure for nested parsing
3. Implement: Lexer → Parser → Resolver → Tests
4. Test and validate before moving to Task 0.2

See [`.todo/improve-template.md`](.todo/improve-template.md) for full details.
