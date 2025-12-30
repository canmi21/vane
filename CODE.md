# Vane Code Organization Analysis

**Purpose:** This document analyzes the current codebase structure, identifies organizational issues, and proposes improvements. This analysis must be completed BEFORE any file restructuring to avoid breaking code references.

**Audience:** AI agent (Claude Code) and project owner for code refactoring planning.

**Last Updated:** 2025-12-29

---

## Table of Contents

1. [Code Statistics](#code-statistics)
2. [Directory Structure Analysis](#directory-structure-analysis)
3. [File Organization Patterns](#file-organization-patterns)
4. [Naming Conventions](#naming-conventions)
5. [Module Dependency Analysis](#module-dependency-analysis)
6. [Code Organization Issues](#code-organization-issues)
7. [Proposed Improvements](#proposed-improvements)
8. [Migration Strategy](#migration-strategy)

---

## Code Statistics

### Overall Metrics

```
Total Lines of Code:    14,455
Total Rust Files:       123
Total Directories:      30 (27 subdirectories + 3 top-level)
Average File Size:      ~118 lines

Largest Files:
  1. protocol/quic/parser.rs           731 lines
  2. terminator/transport/proxy/proxy.rs  470 lines
  3. stack/transport/udp.rs            348 lines
  4. resource/static.rs                343 lines
  5. certs/loader.rs                   298 lines
```

**Observation:** Reasonably sized files (average 118 lines). Largest file (731 lines) is QUIC parser, which is acceptable for complex protocol parsing. No obvious "god files" (>1000 lines).

### Directory Distribution

```
src/
├── common/             6 files     ~500 lines    # Utility modules
├── core/               7 files     ~800 lines    # Bootstrap, daemon
├── middleware/         2 files     ~100 lines    # Request logging
└── modules/          108 files  ~13,055 lines    # Main functionality
```

**Observation:** 88% of code in `modules/` directory. This is expected since modules contain all business logic.

---

## Directory Structure Analysis

### Current Structure

```
src/
├── main.rs                           # Entry point (daemon start)
├── core/                             # System initialization
│   ├── bootstrap.rs                  # Configuration loading
│   ├── daemon.rs                     # Daemon mode (background process)
│   ├── router.rs                     # HTTP router for plugin API
│   ├── response.rs                   # HTTP response utilities
│   ├── root.rs                       # Root endpoint handler
│   └── socket.rs                     # Socket utilities
├── common/                           # Shared utilities
│   ├── getconf.rs                    # Configuration loading
│   ├── getenv.rs                     # Environment variable helpers
│   ├── ip.rs                         # IP address utilities
│   ├── portool.rs                    # Port utilities
│   └── requirements.rs               # Error type definitions
├── middleware/                       # HTTP middleware
│   └── logger.rs                     # Request logging
└── modules/                          # Core functional modules
    ├── stack/                        # Three-layer network stack
    │   ├── transport/                # L4: TCP/UDP
    │   └── protocol/                 # L4+ and L7
    │       ├── carrier/              # L4+: TLS, QUIC
    │       └── application/          # L7: HTTP, Container
    ├── plugins/                      # Plugin system
    │   ├── model.rs                  # Trait definitions
    │   ├── registry.rs               # Plugin registry
    │   ├── loader.rs                 # External plugin loader
    │   ├── external.rs               # External plugin wrapper
    │   ├── handler.rs                # Plugin management API
    │   ├── middleware/               # Internal middleware
    │   │   └── common/               # Common middleware
    │   ├── terminator/               # Internal terminators
    │   │   ├── transport/            # L4/L4+ terminators
    │   │   │   └── proxy/            # Proxy variants
    │   │   ├── response/             # L7 response terminator
    │   │   └── upgrader/             # Protocol upgrade
    │   ├── drivers/                  # External plugin drivers
    │   ├── protocol/                 # Protocol detection plugins
    │   │   ├── tls/                  # TLS ClientHello parsing
    │   │   └── quic/                 # QUIC packet parsing
    │   ├── upstream/                 # FetchUpstream plugin
    │   ├── cgi/                      # CGI plugin
    │   ├── resource/                 # Static file plugin
    │   └── common/                   # Common utilities
    ├── kv/                           # KV store module
    ├── nodes/                        # Upstream nodes registry
    ├── ports/                        # Listener management
    └── certs/                        # Certificate management
```

### Structural Observations

**1. Top-Level Organization:**

```
src/
├── main.rs           ✓ Clear entry point
├── core/             ✓ System initialization separate
├── common/           ✓ Utilities isolated
├── middleware/       ⚠ Only used for HTTP API logging, unclear naming
└── modules/          ✓ Main business logic
```

**Analysis:**
- `core/` and `common/` separation is reasonable (core = initialization, common = utilities)
- `middleware/` contains only HTTP API request logging, not proxy middleware (confusing naming)
- `modules/` is a catchall directory (could be flattened or reorganized)

**2. Module Nesting Depth:**

```
Deepest path: src/modules/plugins/terminator/transport/proxy/
Depth: 6 levels

Common depth: 3-4 levels (reasonable)
```

**Observation:** Nesting depth is acceptable. Most modules are 3-4 levels deep, which is readable.

---

## File Organization Patterns

### Plugin System File Layout

**Problem:** Plugin files scattered across multiple top-level directories

```
Current:
src/modules/plugins/
├── model.rs                          # Trait definitions
├── registry.rs                       # Plugin registry
├── loader.rs                         # External plugin loader
├── external.rs                       # External plugin wrapper
├── handler.rs                        # Plugin management API
├── drivers/                          # External plugin drivers (HTTP, Unix, Command)
├── middleware/                       # Internal middleware
│   └── common/                       # ✗ Only contains ratelimit.rs and matcher.rs
├── terminator/                       # Internal terminators
│   ├── transport/                    # L4/L4+ terminators
│   ├── response/                     # L7 response terminator
│   └── upgrader/                     # Protocol upgrade (actually a terminator)
├── protocol/                         # ✗ Protocol detection plugins (should be middleware?)
│   ├── tls/                          # TLS ClientHello parsing
│   └── quic/                         # QUIC packet parsing
├── upstream/                         # ✗ FetchUpstream plugin (L7 driver, not standalone)
├── cgi/                              # ✗ CGI plugin (L7 driver, not standalone)
├── resource/                         # ✗ Static file plugin (L7 driver, not standalone)
└── common/                           # ✗ Common utilities (duplicate of top-level common?)
```

**Issues:**

1. **Inconsistent Categorization:**
   - `protocol/` contains TLS/QUIC parsing, but these are actually middleware plugins (ProtocolDetect)
   - `upstream/`, `cgi/`, `resource/` are L7 drivers but placed at same level as `middleware/` and `terminator/`

2. **Duplicate "common" Directories:**
   - `src/common/` - Top-level utilities
   - `src/modules/plugins/common/` - Plugin utilities
   - `src/modules/plugins/middleware/common/` - Middleware utilities

   Unclear which to use for new utilities.

3. **Sparse Directories:**
   - `middleware/common/` contains only 2 files (ratelimit.rs, matcher.rs)
   - Should these be in `middleware/` directly?

### Stack Module File Layout

```
Current:
src/modules/stack/
├── transport/                        # L4: TCP/UDP
│   ├── mod.rs
│   ├── flow.rs                       # Flow execution engine
│   ├── proxy.rs                      # Bidirectional forwarding
│   ├── tcp.rs                        # TCP listener and handler
│   ├── udp.rs                        # UDP listener and handler
│   ├── validator.rs                  # Flow validation
│   ├── dispatcher.rs                 # Connection routing
│   ├── balancer.rs                   # Load balancing
│   └── health.rs                     # Health check
└── protocol/                         # L4+ and L7
    ├── carrier/                      # L4+: TLS, QUIC
    │   ├── mod.rs
    │   ├── flow.rs                   # Flow execution engine
    │   ├── tls/                      # TLS carrier
    │   │   ├── mod.rs
    │   │   ├── clienthello.rs        # ClientHello parsing
    │   │   └── passthrough.rs        # TLS passthrough
    │   └── quic/                     # QUIC carrier
    │       ├── mod.rs
    │       ├── session.rs            # Session management
    │       ├── muxer.rs              # Packet multiplexing
    │       ├── virtual_socket.rs     # Virtual socket abstraction
    │       └── frame.rs              # CRYPTO frame parsing
    └── application/                  # L7: HTTP, Container
        ├── mod.rs
        ├── flow.rs                   # Flow execution engine
        ├── container.rs              # Container model
        ├── template.rs               # Template resolution
        └── http/                     # HTTP adapters
            ├── mod.rs
            ├── wrapper.rs            # VaneBody abstraction
            ├── httpx.rs              # HTTP/1.1, HTTP/2 adapter
            └── h3.rs                 # HTTP/3 adapter
```

**Observations:**

✓ **Good:**
- Clear layer separation (transport, carrier, application)
- Each layer has its own `flow.rs` (flow execution engine)
- Protocol-specific code isolated (tls/, quic/, http/)

⚠ **Issues:**
- `protocol/` naming is generic (could be `carrier_and_application/` or split into two top-level modules)
- Carrier and application in same parent directory (different abstraction levels)

### Core Modules File Layout

```
Current:
src/modules/
├── kv/
│   ├── mod.rs                        # KV store type definition
│   └── plugin_output.rs              # Namespacing utilities
├── nodes/
│   ├── mod.rs                        # Node registry
│   ├── model.rs                      # NodeConfig definition
│   └── hotswap.rs                    # Hot-reload logic
├── ports/
│   ├── mod.rs                        # Port registry
│   ├── model.rs                      # PortConfig definition
│   ├── hotswap.rs                    # Hot-reload logic
│   ├── handler.rs                    # Connection handler
│   └── listener.rs                   # TCP/UDP listener
└── certs/
    ├── mod.rs                        # Certificate registry
    ├── model.rs                      # CertificateData definition
    ├── loader.rs                     # Certificate loading/validation
    └── hotswap.rs                    # Hot-reload logic
```

**Observations:**

✓ **Good:**
- Consistent pattern across nodes/, ports/, certs/ (mod.rs, model.rs, hotswap.rs)
- Clear responsibility (each module manages one registry)

⚠ **Potential Issue:**
- Duplicate `hotswap.rs` in nodes/, ports/, certs/ (could extract common hot-reload logic)

---

## Naming Conventions

### File Naming Analysis

**Current Patterns:**

| Pattern | Examples | Count | Consistency |
|---------|----------|-------|-------------|
| `mod.rs` | Every directory | ~30 | ✓ 100% |
| `model.rs` | nodes/model.rs, ports/model.rs | 3 | ✓ Consistent |
| `{feature}.rs` | tcp.rs, udp.rs, flow.rs | ~80 | ✓ Descriptive |
| `{verb}er.rs` | parser.rs, loader.rs, muxer.rs | 10 | ✓ Good |
| `{action}.rs` | hotswap.rs, validator.rs | 5 | ✓ Good |

**Observations:**

✓ **Strengths:**
- Consistent use of `mod.rs` for module entry points
- `model.rs` used for data structure definitions (good convention)
- Descriptive names (tcp.rs, udp.rs, flow.rs)
- Agent nouns (parser.rs, loader.rs) for active components

⚠ **Inconsistencies:**

1. **Plugin File Naming:**
   ```
   plugins/upstream/mod.rs         # FetchUpstream plugin
   plugins/cgi/executor.rs         # CGI plugin
   plugins/resource/static.rs      # Static file plugin
   ```

   Should these all be `mod.rs` or all be named after the plugin?

2. **Protocol Detection:**
   ```
   plugins/protocol/tls/mod.rs     # TLS detection
   plugins/protocol/quic/parser.rs # QUIC detection
   ```

   One uses `mod.rs`, other uses `parser.rs`. Inconsistent.

### Directory Naming Analysis

**Current Patterns:**

| Pattern | Examples | Count | Consistency |
|---------|----------|-------|-------------|
| Singular nouns | core, middleware, kv | 10 | ⚠ Mixed |
| Plural nouns | modules, plugins, nodes, ports, certs | 5 | ⚠ Mixed |
| Compound words | terminator, transport | 5 | ✓ Good |

**Issues:**

1. **Singular vs. Plural Inconsistency:**
   ```
   ✓ plugins/          (plural, contains multiple plugins)
   ✓ nodes/            (plural, contains multiple node configs)
   ✗ middleware/       (singular, but should be "middlewares"?)
   ✗ core/             (singular, abstract concept)
   ```

   **Recommendation:** Use plural for collections (plugins, nodes), singular for concepts (core, stack).

2. **Ambiguous Names:**
   ```
   plugins/middleware/     # Internal middleware plugins
   plugins/terminator/     # Internal terminators
   plugins/drivers/        # External plugin drivers
   plugins/protocol/       # Protocol detection plugins (actually middleware)
   plugins/upstream/       # FetchUpstream plugin (actually L7 driver)
   plugins/cgi/            # CGI plugin (actually L7 driver)
   plugins/resource/       # Static plugin (actually L7 driver)
   ```

   **Problem:** `upstream/`, `cgi/`, `resource/` are L7 drivers but not under `drivers/` (which is for external plugin drivers).

---

## Module Dependency Analysis

### Dependency Graph (High-Level)

```
main.rs
  └─> core/bootstrap.rs
       ├─> common/getconf.rs
       ├─> modules/ports/ (load port configs)
       ├─> modules/nodes/ (load node configs)
       ├─> modules/certs/ (load certificates)
       └─> modules/plugins/ (load external plugins)

modules/ports/handler.rs
  └─> modules/stack/transport/flow.rs
       ├─> modules/plugins/registry.rs (get plugins)
       ├─> modules/kv/ (create KV store)
       └─> modules/stack/transport/proxy.rs (terminators)

modules/stack/transport/flow.rs
  └─> modules/stack/protocol/carrier/flow.rs (on upgrade)
       └─> modules/stack/protocol/application/flow.rs (on upgrade)
            └─> modules/plugins/upstream/ (FetchUpstream)
            └─> modules/plugins/cgi/ (CGI)
            └─> modules/plugins/resource/ (Static)
```

**Observations:**

✓ **Good:**
- Clear dependency hierarchy (main → core → modules)
- No circular dependencies detected
- Stack layers depend on next layer (transport → carrier → application)

⚠ **Issues:**
- Plugins depend on stack (e.g., FetchUpstream imports Container from stack/protocol/application)
- Stack depends on plugins (e.g., flow.rs imports Plugin trait)
- Bidirectional dependency (plugin ↔ stack)

**Potential Problem:** Plugin and stack modules are tightly coupled. Changing Container structure requires updating all L7 plugins.

### Module Coupling

**High Coupling:**

```
modules/plugins/ ↔ modules/stack/
  - plugins/model.rs defines Plugin trait
  - stack/transport/flow.rs uses Plugin trait
  - stack/protocol/application/container.rs defines Container
  - plugins/upstream/mod.rs uses Container
```

**Solution:** Extract interface layer (traits module) to break circular dependency.

**Low Coupling:**

```
modules/kv/          # No dependencies on other modules
modules/nodes/       # Only depends on common/
modules/ports/       # Only depends on stack/
modules/certs/       # Only depends on common/
```

**Observation:** Support modules (kv, nodes, ports, certs) have low coupling (good).

---

## Code Organization Issues

### Issue 1: Plugin System Fragmentation

**Problem:** Plugins scattered across 7 top-level directories:

```
plugins/
├── middleware/        # Internal middleware
├── terminator/        # Internal terminators
├── drivers/           # External plugin drivers
├── protocol/          # Protocol detection (actually middleware)
├── upstream/          # FetchUpstream (actually L7 driver/middleware)
├── cgi/               # CGI (actually L7 driver/middleware)
└── resource/          # Static (actually L7 driver/middleware)
```

**Impact:**
- Difficult to find plugins (is FetchUpstream in `middleware/` or `drivers/` or `upstream/`?)
- Inconsistent categorization (protocol detection vs. rate limiting both middleware, but in different directories)

**Expected Structure:**

```
plugins/
├── core/                  # Plugin system core
│   ├── model.rs           # Trait definitions
│   ├── registry.rs        # Plugin registry
│   ├── loader.rs          # External plugin loader
│   └── external.rs        # External plugin wrapper
├── drivers/               # External plugin drivers (HTTP, Unix, Command)
├── middleware/            # Internal middleware plugins
│   ├── protocol.rs        # Protocol detection (from protocol/)
│   ├── matcher.rs         # Common matching
│   ├── ratelimit.rs       # Rate limiting
│   └── ...
├── terminators/           # Internal terminators
│   ├── abort.rs
│   ├── proxy.rs
│   ├── upgrade.rs
│   └── ...
└── l7/                    # L7-specific plugins
    ├── fetch_upstream.rs  # FetchUpstream driver
    ├── cgi.rs             # CGI driver
    └── static_files.rs    # Static file driver
```

### Issue 2: Duplicate "common" Directories

**Problem:** Three "common" directories with unclear boundaries:

```
src/common/                         # Top-level utilities
  ├── getconf.rs                    # Config loading
  ├── getenv.rs                     # Environment variables
  ├── ip.rs                         # IP utilities
  ├── portool.rs                    # Port utilities
  └── requirements.rs               # Error types

src/modules/plugins/common/         # Plugin utilities (EMPTY - only mod.rs)

src/modules/plugins/middleware/common/  # Middleware utilities
  ├── ratelimit.rs                  # Rate limiting
  └── matcher.rs                    # Pattern matching
```

**Impact:**
- Unclear where to add new utilities
- `plugins/common/` is empty (serves no purpose)
- `middleware/common/` contains only 2 files (could be in `middleware/` directly)

**Recommendation:**
- **Keep:** `src/common/` for system-wide utilities
- **Remove:** `src/modules/plugins/common/` (empty)
- **Flatten:** `middleware/common/ratelimit.rs` → `middleware/ratelimit.rs`

### Issue 3: Stack Module Protocol Ambiguity

**Problem:** `stack/protocol/` contains both L4+ (carrier) and L7 (application), but "protocol" is generic.

```
stack/
├── transport/         # L4
└── protocol/          # ??? (too generic)
    ├── carrier/       # L4+
    └── application/   # L7
```

**Impact:**
- Unclear what "protocol" means (carrier? application? both?)
- Deep nesting (stack/protocol/carrier/quic/)

**Alternative Structure:**

```
stack/
├── transport/         # L4
├── carrier/           # L4+ (move up one level)
└── application/       # L7 (move up one level)
```

OR:

```
stack/
└── layers/
    ├── transport/     # L4
    ├── carrier/       # L4+
    └── application/   # L7
```

### Issue 4: Inconsistent Plugin File Structure

**Problem:** L7 driver plugins use different file structures:

```
plugins/upstream/
├── mod.rs                # FetchUpstream plugin
├── pool.rs               # Connection pooling
└── request.rs            # Request building

plugins/cgi/
├── mod.rs                # Module entry
├── executor.rs           # CGI plugin (main logic)
└── env.rs                # Environment building

plugins/resource/
└── static.rs             # Static plugin (no mod.rs)
```

**Impact:**
- Inconsistent (upstream uses mod.rs, cgi uses executor.rs, resource uses static.rs)
- Difficult to know which file contains plugin struct

**Recommendation:** Standardize:

```
plugins/l7/
├── fetch_upstream/
│   ├── mod.rs            # Plugin struct
│   ├── pool.rs           # Connection pooling
│   └── request.rs        # Request building
├── cgi/
│   ├── mod.rs            # Plugin struct
│   └── env.rs            # Environment building
└── static/
    └── mod.rs            # Plugin struct (or static.rs, but be consistent)
```

### Issue 5: Hotswap Logic Duplication

**Problem:** Nearly identical `hotswap.rs` in nodes/, ports/, certs/

```
nodes/hotswap.rs:
  - watch_directory()
  - reload_on_change()
  - validate_and_swap()

ports/hotswap.rs:
  - watch_directory()
  - reload_on_change()
  - validate_and_swap()

certs/hotswap.rs:
  - watch_directory()
  - reload_on_change()
  - validate_and_swap()
```

**Impact:**
- Code duplication (~300 lines duplicated)
- Bug fixes must be applied to all three files

**Recommendation:** Extract generic hot-reload framework:

```
common/hotswap.rs:
  pub trait HotSwappable {
      fn load(&self, path: &str) -> Result<Self>;
      fn validate(&self) -> Result<()>;
  }

  pub fn watch_and_reload<T: HotSwappable>(path: &str, registry: ArcSwap<T>) { ... }

nodes/mod.rs:
  impl HotSwappable for NodeRegistry { ... }
  watch_and_reload("/etc/vane/nodes", NODES_STATE);
```

### Issue 6: Flow Execution Code Duplication

**Problem:** Three nearly identical `flow.rs` files:

```
stack/transport/flow.rs          # L4 flow execution
stack/protocol/carrier/flow.rs   # L4+ flow execution
stack/protocol/application/flow.rs  # L7 flow execution
```

**Duplication:**
- `execute_recursive()` function logic 95% identical
- Template resolution same
- Plugin lookup same
- Only difference: Plugin trait variant (Middleware vs. L7Middleware)

**Impact:**
- Bug fixes must be applied to all three
- Feature additions (e.g., flow validation) must be implemented thrice

**Recommendation:** Generic flow engine with trait-based plugin execution:

```
stack/flow_engine.rs:
  pub async fn execute_flow<P: PluginExecutor>(
      step: &ProcessingStep,
      context: &mut P::Context,
  ) -> Result<TerminatorResult> { ... }

stack/transport/flow.rs:
  struct L4PluginExecutor;
  impl PluginExecutor for L4PluginExecutor {
      type Context = (KvStore, ConnectionObject);
      async fn execute_plugin(...) { ... }
  }
```

---

## Proposed Improvements

### Improvement 1: Reorganize Plugin Directory

**Current:**
```
plugins/
├── middleware/common/
├── terminator/
├── drivers/
├── protocol/
├── upstream/
├── cgi/
└── resource/
```

**Proposed:**
```
plugins/
├── core/                    # Plugin system infrastructure
│   ├── model.rs             # Trait definitions
│   ├── registry.rs          # Plugin registry
│   ├── loader.rs            # External plugin loader
│   ├── external.rs          # External plugin wrapper
│   └── drivers/             # External plugin drivers
│       ├── http.rs
│       ├── unix.rs
│       └── command.rs
├── middleware/              # Internal middleware plugins (flatten common/)
│   ├── protocol_detect.rs   # Protocol detection
│   ├── matcher.rs           # Pattern matching
│   ├── ratelimit.rs         # Rate limiting
│   └── ...
├── terminators/             # Internal terminators
│   ├── abort.rs
│   ├── proxy/               # Proxy variants
│   │   ├── transparent.rs
│   │   ├── node.rs
│   │   └── domain.rs
│   └── upgrade.rs
└── l7/                      # L7 drivers (middleware that populate Container)
    ├── fetch_upstream/
    │   ├── mod.rs
    │   ├── pool.rs
    │   └── request.rs
    ├── cgi/
    │   ├── mod.rs
    │   └── env.rs
    └── static/
        └── mod.rs
```

**Benefits:**
- Clear categorization (core, middleware, terminators, l7)
- No duplicate "common" directories
- L7 drivers grouped together
- Flatter structure (easier to navigate)

### Improvement 2: Flatten Stack Module

**Current:**
```
stack/
├── transport/
└── protocol/
    ├── carrier/
    └── application/
```

**Proposed:**
```
stack/
├── transport/       # L4
├── carrier/         # L4+ (moved up one level)
├── application/     # L7 (moved up one level)
└── flow/            # Shared flow execution engine
    ├── engine.rs    # Generic flow executor
    ├── executor.rs  # Trait-based plugin execution
    └── validator.rs # Flow validation
```

**Benefits:**
- Clearer layer separation (transport, carrier, application at same level)
- No ambiguous "protocol" directory
- Shared flow engine extracted (no duplication)

### Improvement 3: Extract Hot-Reload Framework

**Current:**
```
nodes/hotswap.rs
ports/hotswap.rs
certs/hotswap.rs
```

**Proposed:**
```
common/hotswap.rs:
  pub trait HotSwappable {
      type Config;
      fn load(path: &str) -> Result<Self::Config>;
      fn validate(config: &Self::Config) -> Result<()>;
  }

  pub fn watch_and_reload<T: HotSwappable>(
      path: &str,
      registry: ArcSwap<T::Config>,
  ) -> Result<()> { ... }

nodes/mod.rs:
  impl HotSwappable for NodeRegistry {
      type Config = DashMap<String, NodeConfig>;
      fn load(path: &str) -> Result<Self::Config> { ... }
  }
```

**Benefits:**
- No code duplication
- Consistent hot-reload behavior
- Easy to add new hot-swappable components

### Improvement 4: Standardize Plugin File Structure

**Current:**
```
upstream/mod.rs              # Plugin struct
cgi/executor.rs              # Plugin struct
resource/static.rs           # Plugin struct
```

**Proposed:**
```
l7/fetch_upstream/mod.rs     # Plugin struct + trait impl
l7/cgi/mod.rs                # Plugin struct + trait impl
l7/static/mod.rs             # Plugin struct + trait impl
```

**Rule:** Plugin struct always in `mod.rs`, helper modules in separate files.

### Improvement 5: Unify Flow Execution

**Current:**
```
stack/transport/flow.rs      # 200 lines, mostly duplicate
stack/protocol/carrier/flow.rs   # 200 lines, mostly duplicate
stack/protocol/application/flow.rs  # 200 lines, mostly duplicate
```

**Proposed:**
```
stack/flow/engine.rs:
  pub async fn execute_flow<E: FlowExecutor>(
      step: &ProcessingStep,
      executor: &E,
      context: &mut E::Context,
  ) -> Result<TerminatorResult> {
      // Generic flow execution logic
  }

stack/transport/executor.rs:
  pub struct L4Executor;
  impl FlowExecutor for L4Executor {
      type Context = (KvStore, ConnectionObject);
      async fn execute_middleware(...) { ... }
      async fn execute_terminator(...) { ... }
  }
```

**Benefits:**
- Single source of truth for flow logic
- Bug fixes apply to all layers
- Easier to add features (e.g., flow timeout)

---

## Migration Strategy

**CRITICAL:** Do NOT restructure files until architecture improvements are designed and approved. Otherwise, code references break and project becomes unmaintainable.

### Phase 1: Document Current State (DONE)
- ✓ Create ARCHITECTURE.md (architecture analysis)
- ✓ Create CODE.md (code organization analysis)

### Phase 2: Design Improvements
1. Review ARCHITECTURE.md and CODE.md with project owner
2. Prioritize improvements (which are critical? which are nice-to-have?)
3. Design detailed refactoring plan for each improvement
4. Get approval before proceeding

### Phase 3: Extract Shared Logic (No File Moves)
1. **Extract flow engine:**
   - Create `stack/flow/engine.rs` with generic flow executor
   - Update transport/flow.rs, carrier/flow.rs, application/flow.rs to use generic engine
   - Test: No behavior change

2. **Extract hot-reload framework:**
   - Create `common/hotswap.rs` with HotSwappable trait
   - Update nodes/, ports/, certs/ to use generic framework
   - Test: Hot-reload still works

3. **Extract template functions:**
   - Create `stack/flow/template.rs` with template function system
   - Update flow engine to use template functions
   - Test: Existing templates resolve correctly

### Phase 4: Reorganize Plugins (File Moves)
1. **Create new directory structure:**
   ```
   mkdir -p src/modules/plugins/core
   mkdir -p src/modules/plugins/l7
   ```

2. **Move files systematically:**
   ```
   # Move core files
   mv src/modules/plugins/model.rs src/modules/plugins/core/
   mv src/modules/plugins/registry.rs src/modules/plugins/core/
   mv src/modules/plugins/loader.rs src/modules/plugins/core/
   mv src/modules/plugins/external.rs src/modules/plugins/core/

   # Move L7 drivers
   mv src/modules/plugins/upstream/ src/modules/plugins/l7/fetch_upstream/
   mv src/modules/plugins/cgi/ src/modules/plugins/l7/
   mv src/modules/plugins/resource/ src/modules/plugins/l7/static/

   # Flatten middleware
   mv src/modules/plugins/middleware/common/ratelimit.rs src/modules/plugins/middleware/
   mv src/modules/plugins/middleware/common/matcher.rs src/modules/plugins/middleware/
   rmdir src/modules/plugins/middleware/common/

   # Rename protocol → middleware/protocol_detect.rs
   # ... (continue for all files)
   ```

3. **Update imports:**
   ```
   # Use find-replace to update all imports
   # Before: use crate::modules::plugins::model::Plugin;
   # After:  use crate::modules::plugins::core::model::Plugin;
   ```

4. **Test after EACH move:**
   ```
   cargo build --all-features
   cargo test
   ```

   **CRITICAL:** Never move multiple files before testing. One failure becomes debugging nightmare.

### Phase 5: Flatten Stack Module (File Moves)
1. **Move carrier and application up one level:**
   ```
   mv src/modules/stack/protocol/carrier/ src/modules/stack/
   mv src/modules/stack/protocol/application/ src/modules/stack/
   rmdir src/modules/stack/protocol/
   ```

2. **Update imports:**
   ```
   # Before: use crate::modules::stack::protocol::carrier::tls;
   # After:  use crate::modules::stack::carrier::tls;
   ```

3. **Test:**
   ```
   cargo build --all-features
   cargo test
   ```

### Phase 6: Update Documentation
1. Update ARCHITECTURE.md with new structure
2. Update CODE.md to reflect completed improvements
3. Update docs/development.md with new file locations
4. Update README.md if necessary

### Phase 7: Validate and Deploy
1. Run full test suite (unit + integration)
2. Test hot-reload (manually modify configs)
3. Load test (ensure no performance regression)
4. Deploy to staging
5. Monitor for issues
6. Deploy to production

---

## File Naming Recommendations

### General Principles

1. **Use `mod.rs` for module entry points:**
   ```
   ✓ plugins/middleware/mod.rs
   ✗ plugins/middleware/middleware.rs
   ```

2. **Use descriptive names for feature files:**
   ```
   ✓ tcp.rs, udp.rs, flow.rs
   ✗ handler.rs (too generic)
   ```

3. **Use `model.rs` for data structures:**
   ```
   ✓ nodes/model.rs (contains NodeConfig)
   ✓ ports/model.rs (contains PortConfig)
   ```

4. **Use agent nouns for active components:**
   ```
   ✓ parser.rs, loader.rs, validator.rs
   ✗ parse.rs, load.rs, validate.rs
   ```

5. **Avoid redundant naming:**
   ```
   ✗ plugins/plugins.rs
   ✗ middleware/middleware.rs
   ✓ plugins/mod.rs
   ```

### Specific Recommendations

**Plugin Files:**
```
✓ plugins/core/model.rs          # Trait definitions
✓ plugins/middleware/protocol_detect.rs
✓ plugins/terminators/abort.rs
✓ plugins/l7/fetch_upstream/mod.rs

✗ plugins/middleware/protocol/mod.rs  # Too nested
✗ plugins/upstream.rs                  # Unclear if module or file
```

**Stack Files:**
```
✓ stack/transport/flow.rs
✓ stack/carrier/tls/clienthello.rs
✓ stack/application/container.rs

✗ stack/protocol/carrier/tls/ch.rs   # Abbreviation unclear
✗ stack/l7/http.rs                    # "l7" abbreviation
```

---

## Complexity Metrics

### Cyclomatic Complexity (Estimated)

**High Complexity Files** (likely >15 branches):

1. `plugins/protocol/quic/parser.rs` (731 lines) - Packet parsing state machine
2. `plugins/terminator/transport/proxy/proxy.rs` (470 lines) - Proxy logic with multiple modes
3. `plugins/resource/static.rs` (343 lines) - File serving with range requests, compression
4. `certs/loader.rs` (298 lines) - Certificate loading with multiple formats
5. `core/bootstrap.rs` (296 lines) - System initialization with error handling

**Recommendation:** Profile these files for refactoring opportunities. Consider extracting complex logic into separate functions or modules.

### Coupling Metrics

**Highly Coupled Modules:**

```
plugins/ ↔ stack/     # Plugin trait <-> Container
stack/ ↔ ports/       # Flow execution <-> Connection handling
core/ → (all modules) # Bootstrap depends on everything
```

**Loosely Coupled Modules:**

```
kv/                   # No dependencies
common/               # Only standard library
nodes/                # Only depends on common/
```

**Recommendation:** Extract interface layer (traits) to decouple plugins and stack.

---

## Summary

### Current State Assessment

**Strengths:**
- ✓ Reasonable file sizes (avg 118 lines, largest 731 lines)
- ✓ Clear separation between layers (transport, carrier, application)
- ✓ Consistent use of `mod.rs` for module entry points
- ✓ Good module isolation (kv, nodes, ports, certs independent)

**Weaknesses:**
- ✗ Plugin system fragmented (7 top-level directories)
- ✗ Duplicate "common" directories (3 total)
- ✗ Flow execution code duplicated (3 identical files)
- ✗ Hot-reload logic duplicated (3 identical files)
- ✗ Inconsistent plugin file structure (mod.rs vs. named files)
- ✗ Ambiguous "protocol" directory (contains carrier + application)

### Recommended Priorities

**Priority 1 (Critical):**
1. Extract flow execution engine (eliminate duplication)
2. Extract hot-reload framework (eliminate duplication)

**Priority 2 (High):**
3. Reorganize plugin directory (improve discoverability)
4. Flatten stack module (clearer structure)

**Priority 3 (Medium):**
5. Standardize plugin file structure (consistency)
6. Remove duplicate "common" directories

**Priority 4 (Low):**
7. Refactor high-complexity files (maintainability)
8. Extract interface layer (reduce coupling)

### Next Steps

1. **Review with project owner:**
   - Agree on priority order
   - Identify any organizational constraints

2. **Detailed design:**
   - Design flow engine API
   - Design hot-reload framework API
   - Plan migration for each phase

3. **Implementation:**
   - NEVER move files until design approved
   - Test after EACH change
   - Update documentation as changes complete

**Remember:** Architecture improvements must precede code reorganization. Otherwise, we break the codebase trying to fix it.
