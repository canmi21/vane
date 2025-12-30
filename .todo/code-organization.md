# Priority 3: Code Organization Tasks (Phase III - 面子工程)

These tasks restructure the source folder hierarchy and must be done AFTER Phase I and Phase II are complete.

**Critical Rule:** Only move files AFTER core architecture is stable. Moving files during active refactoring causes merge conflicts.

---

## Task 3.1: Reorganize Plugin Directory

**Status:** Planned
**Blocker:** Must complete Phase I improvements first

**Target Structure:**
```
plugins/
├── core/           # Plugin system infrastructure
├── drivers/        # External plugin drivers
├── middleware/     # Internal middleware (flatten common/)
├── terminators/    # Internal terminators
└── l7/             # L7 drivers (upstream, cgi, static)
```

---

## Task 3.2: Flatten Stack Module

**Status:** Planned
**Blocker:** Must complete Phase I improvements first

**Target Structure:**
```
stack/
├── transport/      # L4
├── carrier/        # L4+ (move up from protocol/carrier/)
├── application/    # L7 (move up from protocol/application/)
└── flow/           # Shared flow engine (if extracted)
```

---

## Task 3.3: Standardize Plugin File Structure

**Status:** Planned
**Blocker:** Must complete 3.1 first

**Rule:** Plugin struct always in `mod.rs`

---

## Additional Tasks

- Update all import paths
- Verify no broken references
- Update documentation with new paths
- Run full test suite to confirm nothing broke
