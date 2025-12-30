# Overall Implementation Roadmap

**Codebase Current State:** ✅ Good to go (no compiler errors, no half-finished features, ready for refactoring)

The project will be implemented in 4 major phases:

## Phase I: Core Architecture Implementation (里子工程)

**Goal:** Implement foundational architecture changes that affect core design

**Tasks:**
- ✅ 0.2 Design (Generic Container + Middleware split)
- 0.1 Correct Vane positioning in documentation
- 1.5 Template system upgrade (nested parsing, concatenation, hijacking support)
- 0.2 Implementation: Refactor L7 Container to generic (5 phases)
- 1.2 Extract flow execution engine
- 1.3 Extract hot-reload framework

**Why First:**
- These changes affect the entire codebase structure
- Delaying them would mean duplicate work (fix bugs → refactor → fix bugs again)
- New architecture will have different optimization points and potential issues
- Foundation must be solid before building on top

**Milestone:** Core architecture is generic, extensible, and supports multiple protocols

---

## Phase II: Code Quality & Optimization (查漏补缺)

**Goal:** Identify and fix issues in the NEW architecture

**Tasks:**
- 0.3 Architecture vulnerability and design issue scan
- 0.4 L4 traditional configuration strategy
- 1.1 Rust feature flags support
- 1.4 Flow validation framework
- Fix identified security vulnerabilities
- Optimize performance bottlenecks
- Refactor high-complexity files

**Why Second:**
- Phase I changes will introduce new code patterns and potential issues
- Scanning for vulnerabilities on the OLD architecture would waste effort
- Optimization priorities differ in generic vs monolithic architecture
- Flow validation needs to work with the new trait system

**Milestone:** Codebase is secure, optimized, and validated

---

## Phase III: Code Organization (面子工程)

**Goal:** Restructure source folder hierarchy for maintainability

**Tasks:**
- 3.1 Reorganize plugin directory structure
- 3.2 Flatten stack module hierarchy
- 3.3 Standardize plugin file structure
- Update all import paths
- Verify no broken references

**Why Third:**
- Only move files AFTER core architecture is stable
- Moving files during active refactoring causes merge conflicts
- Clean folder structure makes the new architecture obvious to contributors
- Final "polish" that makes codebase maintainable long-term

**Milestone:** Source code organization is clear, logical, and matches architecture

---

## Phase IV: Documentation & User Guides (文档完善)

**Goal:** Create comprehensive documentation based on FINAL codebase structure

**Tasks:**
- Rewrite docs/overview.md index based on new source structure
- Update ARCHITECTURE.md to reflect implemented changes
- Update CODE.md with new folder structure
- Create web-based user documentation
- Create developer documentation
- Create protocol extension guide (how to add new L7 protocols)
- Update examples to use new configuration patterns

**Why Last:**
- Documentation must reflect ACTUAL codebase, not planned changes
- Screenshots, file paths, line numbers all depend on final structure
- User guides need to reference stable APIs
- Developer docs need accurate source tree navigation

**Milestone:** Complete, accurate documentation for users and contributors

---

## Decision Rationale: Phase I First

**Recommendation: Start with Phase I (Architecture Implementation)**

**Rationale:**
1. **Avoid Duplicate Work:**
   - Scanning for issues now → Implement Phase I → Many issues become irrelevant or reappear differently
   - Better: Implement Phase I → Scan the NEW architecture → Fix issues once

2. **Current Codebase is Stable:**
   - No compiler errors
   - No half-finished features
   - Good baseline for refactoring

3. **Architecture Changes are Foundational:**
   - Container generalization affects ALL L7 code
   - Middleware split affects plugin system
   - Template system affects flow execution
   - These changes will reshape what "code quality" means

4. **Incremental Implementation:**
   - Phase I can be done in small, testable chunks
   - Each phase has clear acceptance criteria
   - Compiler will catch breaking changes immediately

**Alternative: Start with Phase II (Scan First)**

**If you prefer to scan first:**
- Pro: Understand current issues before making changes
- Pro: Can fix critical bugs immediately
- Con: Fixes might need to be rewritten after Phase I
- Con: Optimization insights may not apply to new architecture
