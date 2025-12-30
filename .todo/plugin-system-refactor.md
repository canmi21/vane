# Task 0.2.2: Plugin System Refactoring

**Status**: Planned (awaiting Task 1.2 completion)

**Goal**: Refactor the plugin system to distinguish between Generic and Protocol-Specific middleware, and enforce internal-only constraints for protocol-specific plugins.

**Dependencies**:
- Task 0.2.1: Container protocol extension (✅ Completed)
- Task 1.2: Extract flow execution engine (⏳ Recommended to do first)

---

## Background

### Current Problem

All L7 plugins use the same trait interface:

```rust
pub trait L7Middleware: Plugin {
    async fn execute(&self, context: &mut (dyn Any + Send), inputs: ResolvedInputs)
        -> MiddlewareOutput;
}
```

This creates several issues:
1. Generic middleware (CommonMatch, RateLimit) and protocol-specific middleware (FetchUpstream) use the same interface
2. No compile-time distinction between plugins that should be externally loadable vs internal-only
3. No enforcement of stream vs non-stream middleware

---

## Core Concept Distinction

### Generic Middleware (Universal)

**Characteristics**:
- Input: Only through template system `{{...}}`
- Data sources: KV Store + Template Hijacking (protocol-specific, e.g., `{{req.body}}` triggers HTTP lazy-buffer)
- Constraints:
  - Does NOT support stream types
  - Does NOT directly access L7 Container
  - Can be internal (Rust) OR external (HTTP/Unix/Exec driver)
  - Can be stateful or stateless
- Examples: CommonMatch, RateLimit, ProtocolDetect, external plugins

**Why can be external?**
- Only processes JSON-serializable inputs/outputs
- No direct memory access to Container
- No Rust-specific types (OnUpgrade, streams)

### Protocol-Specific Middleware (Fine-Grained)

**Characteristics**:
- Protocol binding: Only usable in specific protocols (e.g., `l7_http_fetchupstream`)
- Key capabilities:
  - DOES support stream types (core difference)
  - CAN directly access L7 Container (core difference)
  - CAN access protocol-specific fields (client_upgrade, upstream_upgrade)
  - MUST be internal (Rust implementation only)
- Examples: FetchUpstream (HTTP), future DnsResolver (DNS)

**Why internal-only?**
- Needs to manipulate Rust Container memory directly
- Handles stream types (zero-copy streaming)
- Accesses protocol-specific Rust types (OnUpgrade is a Rust future)
- Cannot serialize streams across process boundaries without losing zero-copy

### The Boundary

```
Generic Middleware: Template input only → No stream → No Container access → Can be external
Protocol-Specific: Direct Container access → Stream support → Protocol-bound → Internal only
```

---

## Proposed Trait System

### New Trait Definitions

```rust
// ============ Generic Middleware (Externally Loadable) ============
#[async_trait]
pub trait GenericL7Middleware: Plugin {
    /// Generic middleware only processes JSON inputs/outputs.
    /// Does not access Container, thus can be called via IPC to external processes.
    async fn execute(&self, inputs: ResolvedInputs) -> MiddlewareOutput;
}

// ============ HTTP Protocol-Specific Middleware (Internal Only) ============
#[async_trait]
pub trait HttpMiddleware: Plugin {
    /// HTTP-specific middleware directly manipulates Container.
    /// Must be built-in Rust implementation, cannot be external.
    async fn execute(&self, container: &mut Container, inputs: ResolvedInputs)
        -> MiddlewareOutput;
}

// ============ DNS Protocol-Specific Middleware (Future) ============
#[async_trait]
pub trait DnsMiddleware: Plugin {
    async fn execute(&self, container: &mut Container, inputs: ResolvedInputs)
        -> MiddlewareOutput;
}

// ============ Legacy Fallback (Backward Compatibility) ============
#[async_trait]
pub trait L7Middleware: Plugin {
    async fn execute(&self, context: &mut (dyn Any + Send), inputs: ResolvedInputs)
        -> MiddlewareOutput;
}
```

### Plugin Metadata Extension

```rust
pub struct PluginMeta {
    pub name: String,
    pub category: PluginCategory,
    pub can_be_external: bool,  // New field
}

pub enum PluginCategory {
    GenericL7,      // Generic L7 (can be external)
    HttpSpecific,   // HTTP protocol-specific (internal only)
    DnsSpecific,    // DNS protocol-specific (internal only)
    L4Transport,    // L4 layer
    L4pCarrier,     // L4+ layer
}

impl PluginCategory {
    pub fn can_be_external(&self) -> bool {
        matches!(self, PluginCategory::GenericL7)
    }
}
```

---

## Implementation Phases

### Phase 1: Define New Traits and Metadata (3-4 hours)

**Files to modify**:
- `src/modules/plugins/model.rs` - Add new traits
- `src/modules/plugins/registry.rs` - Add PluginCategory enum and metadata

**Changes**:
1. Define `GenericL7Middleware`, `HttpMiddleware` traits
2. Add `PluginCategory` enum
3. Update `PluginMeta` to include `can_be_external: bool`
4. Keep `L7Middleware` for backward compatibility

### Phase 2: Flow Engine Dispatch Logic (4-6 hours)

**Files to modify**:
- `src/modules/stack/protocol/application/flow.rs` (or extracted flow engine module)

**Changes**:
```rust
async fn execute_plugin(plugin: &Arc<dyn Plugin>, container: &mut Container, inputs: HashMap<String, Value>) {
    // 1. Resolve template inputs (all plugins need this)
    let resolved_inputs = {
        let mut context = L7Context { container };
        resolve_inputs(&inputs, &mut context).await
    };

    // 2. Dispatch based on plugin type
    if let Some(generic) = plugin.as_any().downcast_ref::<dyn GenericL7Middleware>() {
        // Generic middleware - only pass resolved inputs, no Container
        let output = generic.execute(resolved_inputs).await?;

        // Generic middleware can only modify KV
        for (k, v) in output.kv_updates {
            container.kv.insert(k, v);
        }

    } else if let Some(http_specific) = plugin.as_any().downcast_ref::<dyn HttpMiddleware>() {
        // HTTP protocol-specific - pass full Container
        let output = http_specific.execute(container, resolved_inputs).await?;

    } else {
        // Fallback to legacy interface (backward compatibility)
        let context: &mut (dyn Any + Send) = container;
        plugin.execute(context, resolved_inputs).await?;
    }
}
```

### Phase 3: Plugin Registry Updates (3-4 hours)

**Files to modify**:
- `src/modules/plugins/registry.rs`

**Changes**:
1. Add `register_generic()` method for generic middleware
2. Add `register_http()` method for HTTP-specific middleware
3. Update registration to track `PluginCategory`
4. Validate plugin category at registration time

### Phase 4: External Plugin Loader Constraints (2-3 hours)

**Files to modify**:
- `src/modules/plugins/loader.rs`

**Changes**:
```rust
pub async fn load_external_plugin(...) -> Result<Arc<dyn Plugin>> {
    // Check: only generic middleware can be external
    let category = detect_plugin_category(&plugin_config)?;

    if !category.can_be_external() {
        return Err(anyhow!(
            "Plugin '{}' is protocol-specific and must be built-in",
            plugin_name
        ));
    }

    // Continue loading external plugin (HTTP/Unix/Exec)
}
```

### Phase 5: Migrate Existing Plugins (5-6 hours)

**Generic middleware to migrate** (~5 plugins):
- CommonMatch
- RateLimit
- ProtocolDetect
- (Other generic middleware)

**HTTP-specific middleware to migrate** (~3 plugins):
- FetchUpstream
- (Other HTTP-specific plugins)

**Changes per plugin**:
- Implement new trait (`GenericL7Middleware` or `HttpMiddleware`)
- Remove `L7Middleware` implementation (or keep for backward compatibility)
- Update signature to match new trait

### Phase 6: Testing and Validation (4-6 hours)

**Test scenarios**:
1. Generic middleware works without Container access
2. HTTP-specific middleware can access Container and streams
3. External plugin loader rejects protocol-specific plugins
4. Flow engine correctly dispatches to different plugin types
5. WebSocket upgrade still works (HTTP-specific functionality)
6. Integration tests pass for all layers

---

## Work Estimate

| Phase | Files | Hours |
|-------|-------|-------|
| 1. Trait definitions | 2 | 3-4 |
| 2. Flow engine dispatch | 1 | 4-6 |
| 3. Registry updates | 1 | 3-4 |
| 4. External loader constraints | 1 | 2-3 |
| 5. Migrate plugins | 8 | 5-6 |
| 6. Testing | N/A | 4-6 |
| **Total** | **~13** | **21-29** |

---

## Dependencies and Ordering

### Why Task 1.2 Should Be Done First

**Current state**:
- Flow engine logic is in `application/flow.rs` (200+ lines, complex)
- Tightly coupled with Container and plugin execution

**If we do Task 1.2 first**:
1. Extract flow engine into separate module (`src/modules/flow/` or `src/modules/stack/engine/`)
2. Clean interface between flow engine and plugins
3. Easier to modify dispatch logic in Phase 2

**If we do Task 0.2.2 first**:
1. Need to modify flow engine in `application/flow.rs`
2. When doing Task 1.2 later, need to refactor again
3. Duplicated work

### Recommended Order

```
1. Task 1.2: Extract flow execution engine (recommended next)
2. Task 1.3: Extract hot-reload framework
3. Task 0.2.2: Plugin system refactoring (this task)
```

---

## Migration Strategy

### Backward Compatibility

Keep legacy `L7Middleware` trait as fallback:
- Existing plugins continue to work
- New plugins use new traits
- Gradual migration over time

### Validation Rules

At plugin registration:
```rust
if plugin.is_protocol_specific() && config.source == "external" {
    return Err("Protocol-specific plugins must be built-in");
}
```

At configuration load time:
```rust
if flow_uses_http_specific_plugin() && protocol != "http" {
    return Err("Plugin 'FetchUpstream' can only be used in HTTP flows");
}
```

---

## Success Criteria

1. Generic middleware cannot access Container
2. Protocol-specific middleware can access Container and streams
3. External plugin loader rejects protocol-specific plugins
4. Flow engine correctly dispatches based on plugin type
5. All existing functionality preserved (WebSocket, streaming, etc.)
6. Integration tests pass
