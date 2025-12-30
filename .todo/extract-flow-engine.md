# Task 1.2: Extract Flow Execution Engine

**Status:** Ready to Implement (Phase I)

**User Decision:** Confirmed Plan A - Extract unified flow engine with ExecutionContext abstraction

**Investigation Complete:** 2025-12-30 - Comprehensive analysis of L4/L4+/L7 flow systems

**Goal:** Extract duplicated flow execution logic (~600 lines) into a unified engine module

---

## Investigation Summary

### Core Finding: 95% Code Duplication

All three layers (L4, L4+, L7) use **identical** flow execution logic:

| Layer | File | Lines | Core Logic |
|-------|------|-------|-----------|
| L4 Transport | `stack/transport/flow.rs` | ~125 | Recursive step execution |
| L4+ Carrier | `stack/protocol/carrier/flow.rs` | ~125 | **Same as L4** |
| L7 Application | `stack/protocol/application/flow.rs` | ~140 | **Same + L7Plugin support** |

**Shared Algorithm**:
```
1. Parse ProcessingStep (single plugin + instance)
2. Resolve template inputs ({{...}})
3. Get plugin from registry
4. Middleware dispatch → branch to next step
5. Terminator dispatch → return result
6. KV scoping (plugin.{path}.{plugin_name}.{key})
```

**Only Differences**:
- **L4/L4+**: `SimpleContext { kv }` → KV-only template resolution
- **L7**: `L7Context { container }` → Template hijacking (HTTP headers/body)
- **L7**: Try `L7Plugin` traits first, fallback to standard plugins

---

## Plan A: Unified Flow Engine with ExecutionContext

### Architecture Design

#### New Module Structure

```
src/modules/flow/
├── mod.rs              # Public API
├── engine.rs           # Core flow execution engine
├── context.rs          # ExecutionContext trait + implementations
└── key_scoping.rs      # KV key formatting (moved from kv/plugin_output.rs)
```

#### Core Abstraction: ExecutionContext Trait

**Purpose**: Abstract differences between L4/L4+/L7 contexts

```rust
// src/modules/flow/context.rs

use std::any::Any;
use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::Value;

use crate::modules::kv::KvStore;

/// Execution context abstraction for flow engine.
///
/// Different layers provide different contexts:
/// - L4/L4+: KV Store only (TransportContext)
/// - L7: Container with headers/body (ApplicationContext)
#[async_trait]
pub trait ExecutionContext: Send {
    /// Get mutable reference to KV store (all layers have this)
    fn kv_mut(&mut self) -> &mut KvStore;

    /// Resolve template inputs using layer-specific logic
    ///
    /// L4/L4+: SimpleContext (KV lookup only)
    /// L7: L7Context (hijacking support for {{req.body}}, {{res.header.*}}, etc.)
    async fn resolve_inputs(&mut self, inputs: &HashMap<String, Value>)
        -> HashMap<String, Value>;

    /// Get type-erased context for plugins that need it
    ///
    /// Some terminators need access to ConnectionObject or Container.
    /// This provides the underlying context as `&mut (dyn Any + Send)`.
    fn as_any_mut(&mut self) -> &mut (dyn Any + Send);
}
```

#### Implementations

**L4/L4+ Transport Context**:
```rust
// src/modules/flow/context.rs

use crate::modules::template::{context::SimpleContext, resolve_inputs};

/// Transport context for L4 and L4+ layers.
///
/// Only has KV store, no protocol-specific data.
pub struct TransportContext<'a> {
    pub kv: &'a mut KvStore,
}

#[async_trait]
impl<'a> ExecutionContext for TransportContext<'a> {
    fn kv_mut(&mut self) -> &mut KvStore {
        self.kv
    }

    async fn resolve_inputs(&mut self, inputs: &HashMap<String, Value>)
        -> HashMap<String, Value> {
        // Use SimpleContext (KV lookup only, no hijacking)
        let mut simple_ctx = SimpleContext { kv: self.kv };
        crate::modules::template::resolve_inputs(inputs, &mut simple_ctx).await
    }

    fn as_any_mut(&mut self) -> &mut (dyn Any + Send) {
        self.kv as &mut (dyn Any + Send)
    }
}
```

**L7 Application Context**:
```rust
// src/modules/flow/context.rs

use crate::modules::{
    stack::protocol::application::container::Container,
    template::{context::L7Context, resolve_inputs},
};

/// Application context for L7 layer.
///
/// Contains full Container with headers, body, protocol data.
pub struct ApplicationContext<'a> {
    pub container: &'a mut Container,
}

#[async_trait]
impl<'a> ExecutionContext for ApplicationContext<'a> {
    fn kv_mut(&mut self) -> &mut KvStore {
        &mut self.container.kv
    }

    async fn resolve_inputs(&mut self, inputs: &HashMap<String, Value>)
        -> HashMap<String, Value> {
        // Use L7Context (supports hijacking)
        let mut l7_ctx = L7Context { container: self.container };
        crate::modules::template::resolve_inputs(inputs, &mut l7_ctx).await
    }

    fn as_any_mut(&mut self) -> &mut (dyn Any + Send) {
        self.container as &mut (dyn Any + Send)
    }
}
```

#### Unified Flow Engine

```rust
// src/modules/flow/engine.rs

use std::collections::HashMap;

use anyhow::{anyhow, Result};
use serde_json::Value;

use crate::modules::{
    plugins::{
        model::{MiddlewareOutput, Plugin, PluginInstance, ProcessingStep, TerminatorResult},
        registry,
    },
    stack::protocol::model::ConnectionObject,
};

use super::{context::ExecutionContext, key_scoping};

/// Execute a flow starting from the given step.
///
/// This is the unified entry point for all layers (L4, L4+, L7).
///
/// # Parameters
/// - `step`: The ProcessingStep to execute
/// - `context`: Layer-specific execution context
/// - `flow_path`: Current flow path for KV scoping (empty string at start)
///
/// # Returns
/// TerminatorResult when flow completes
pub async fn execute<C: ExecutionContext>(
    step: &ProcessingStep,
    context: &mut C,
    flow_path: String,
) -> Result<TerminatorResult> {
    execute_recursive(step, context, flow_path).await
}

/// Recursive flow execution (internal)
async fn execute_recursive<C: ExecutionContext>(
    step: &ProcessingStep,
    context: &mut C,
    flow_path: String,
) -> Result<TerminatorResult> {
    // 1. Parse step (exactly one plugin per step)
    if step.len() != 1 {
        return Err(anyhow!(
            "Invalid step: expected exactly 1 plugin, found {}",
            step.len()
        ));
    }

    let (plugin_name, instance) = step
        .iter()
        .next()
        .ok_or_else(|| anyhow!("Empty processing step"))?;

    // 2. Resolve template inputs (delegated to context)
    let resolved_inputs = context.resolve_inputs(&instance.input).await;

    // 3. Get plugin from registry
    let plugin = registry::get_plugin(plugin_name)
        .ok_or_else(|| anyhow!("Plugin '{}' not found in registry", plugin_name))?;

    // 4. Try middleware dispatch
    if let Some(middleware) = plugin.as_middleware() {
        let output = middleware.execute(resolved_inputs).await?;

        // Store KV updates with scoped keys
        if let Some(updates) = output.store {
            let kv = context.kv_mut();
            for (raw_key, value) in updates {
                let scoped_key =
                    key_scoping::format_scoped_key(&flow_path, plugin_name, &raw_key);
                kv.insert(scoped_key, value);
            }
        }

        // Branch to next step based on output
        if let Some(next_step) = instance.output.get(output.branch.as_ref()) {
            let next_path =
                key_scoping::next_path(&flow_path, plugin_name, output.branch.as_ref());
            return Box::pin(execute_recursive(next_step, context, next_path)).await;
        } else {
            return Err(anyhow!(
                "Flow stalled at '{}': branch '{}' not configured in output",
                plugin_name,
                output.branch
            ));
        }
    }

    // 5. Try terminator dispatch
    if let Some(terminator) = plugin.as_terminator() {
        let kv = context.kv_mut();

        // Extract ConnectionObject from context (placeholder for L7)
        let conn = ConnectionObject::Virtual(format!("Managed by {}", plugin_name));

        let result = terminator.execute(resolved_inputs, kv, conn).await?;

        // Update flow path for upgrades
        match result {
            TerminatorResult::Finished => Ok(TerminatorResult::Finished),
            TerminatorResult::Upgrade {
                protocol,
                conn,
                parent_path: _,
            } => {
                let upgrade_path =
                    key_scoping::next_path(&flow_path, plugin_name, &protocol);
                Ok(TerminatorResult::Upgrade {
                    protocol,
                    conn,
                    parent_path: upgrade_path,
                })
            }
        }
    } else {
        Err(anyhow!(
            "Plugin '{}' is neither Middleware nor Terminator",
            plugin_name
        ))
    }
}
```

#### L7-Specific Extension

For L7, we need to handle `L7Middleware` and `L7Terminator`:

```rust
// src/modules/flow/engine.rs (L7 variant)

/// Execute L7 flow with support for L7-specific plugins.
///
/// This variant tries L7Plugin traits first, then falls back to standard plugins.
pub async fn execute_l7(
    step: &ProcessingStep,
    container: &mut Container,
    flow_path: String,
) -> Result<TerminatorResult> {
    execute_recursive_l7(step, container, flow_path).await
}

async fn execute_recursive_l7(
    step: &ProcessingStep,
    container: &mut Container,
    flow_path: String,
) -> Result<TerminatorResult> {
    // 1-2. Parse step and resolve inputs (same as generic execute)
    let (plugin_name, instance) = step.iter().next().unwrap();

    let resolved_inputs = {
        let mut context = ApplicationContext { container };
        context.resolve_inputs(&instance.input).await
    };

    let plugin = registry::get_plugin(plugin_name)?;

    // 3. Try L7-specific middleware FIRST
    let output_result: Option<Result<MiddlewareOutput>> =
        if let Some(l7_middleware) = plugin.as_l7_middleware() {
            // L7Middleware has direct Container access
            Some(l7_middleware.execute_l7(container, resolved_inputs.clone()).await)
        } else if let Some(middleware) = plugin.as_middleware() {
            // Fallback to standard Middleware
            Some(middleware.execute(resolved_inputs.clone()).await)
        } else {
            None
        };

    if let Some(result) = output_result {
        let output = result?;

        // Store KV updates
        if let Some(updates) = output.store {
            for (k, v) in updates {
                let scoped_key = key_scoping::format_scoped_key(&flow_path, plugin_name, &k);
                container.kv.insert(scoped_key, v);
            }
        }

        // Branch to next step
        if let Some(next_step) = instance.output.get(output.branch.as_ref()) {
            let next_path = key_scoping::next_path(&flow_path, plugin_name, output.branch.as_ref());
            return Box::pin(execute_recursive_l7(next_step, container, next_path)).await;
        } else {
            return Err(anyhow!(
                "Flow stalled at '{}': branch '{}' not configured",
                plugin_name,
                output.branch
            ));
        }
    }

    // 4. Try L7-specific terminator FIRST
    if let Some(l7_terminator) = plugin.as_l7_terminator() {
        return l7_terminator.execute_l7(container, resolved_inputs).await;
    }

    if let Some(terminator) = plugin.as_terminator() {
        let conn = ConnectionObject::Virtual("L7_Managed_Context".into());
        return terminator.execute(resolved_inputs, &mut container.kv, conn).await;
    }

    Err(anyhow!(
        "Plugin '{}' type mismatch: Expected Middleware/Terminator",
        plugin_name
    ))
}
```

---

## Implementation Phases

### Phase 1: Create Flow Module (2-3 hours)

**Tasks**:
1. Create `src/modules/flow/` directory
2. Create `mod.rs` with public API exports
3. Create `context.rs` with `ExecutionContext` trait
4. Implement `TransportContext` for L4/L4+
5. Implement `ApplicationContext` for L7

**Files Created**:
- `src/modules/flow/mod.rs`
- `src/modules/flow/context.rs`

**Success Criteria**:
- `cargo check` passes
- No duplicate definitions

---

### Phase 2: Implement Unified Engine (3-4 hours)

**Tasks**:
1. Create `engine.rs` with generic `execute()` function
2. Implement `execute_recursive()` with full flow logic
3. Move KV scoping functions from `kv/plugin_output.rs` to `flow/key_scoping.rs`
4. Implement `execute_l7()` variant for L7-specific plugins

**Files Created**:
- `src/modules/flow/engine.rs`
- `src/modules/flow/key_scoping.rs`

**Success Criteria**:
- Generic flow engine compiles
- All helper functions moved

---

### Phase 3: Migrate L4 Transport (2-3 hours)

**Tasks**:
1. Update `stack/transport/flow.rs` to use unified engine
2. Replace local logic with `flow::engine::execute()`
3. Create `TransportContext` wrapper
4. Remove duplicated code

**Before**:
```rust
// stack/transport/flow.rs (~125 lines)
pub async fn execute(
    step: &ProcessingStep,
    kv: &mut KvStore,
    conn: ConnectionObject,
) -> Result<TerminatorResult> {
    // 125 lines of duplicated logic
}
```

**After**:
```rust
// stack/transport/flow.rs (~10 lines)
use crate::modules::flow::{engine, context::TransportContext};

pub async fn execute(
    step: &ProcessingStep,
    kv: &mut KvStore,
    conn: ConnectionObject,
) -> Result<TerminatorResult> {
    let mut context = TransportContext { kv };
    engine::execute(step, &mut context, String::new()).await
}
```

**Success Criteria**:
- L4 flow tests pass
- `cargo check` passes
- Reduced from ~125 lines to ~10 lines

---

### Phase 4: Migrate L4+ Carrier (2-3 hours)

**Tasks**:
1. Update `stack/protocol/carrier/flow.rs` to use unified engine
2. Handle `parent_path` parameter correctly
3. Create `TransportContext` wrapper (same as L4)
4. Remove duplicated code

**Before**:
```rust
// carrier/flow.rs (~125 lines)
pub async fn execute(
    step: &ProcessingStep,
    kv: &mut KvStore,
    conn: ConnectionObject,
    parent_path: String,
) -> Result<TerminatorResult> {
    // 125 lines of duplicated logic
}
```

**After**:
```rust
// carrier/flow.rs (~10 lines)
use crate::modules::flow::{engine, context::TransportContext};

pub async fn execute(
    step: &ProcessingStep,
    kv: &mut KvStore,
    conn: ConnectionObject,
    parent_path: String,
) -> Result<TerminatorResult> {
    let mut context = TransportContext { kv };
    engine::execute(step, &mut context, parent_path).await
}
```

**Success Criteria**:
- L4+ flow tests pass
- `cargo check` passes
- Reduced from ~125 lines to ~10 lines

---

### Phase 5: Migrate L7 Application (3-4 hours)

**Tasks**:
1. Update `stack/protocol/application/flow.rs` to use L7 engine
2. Use `engine::execute_l7()` variant
3. Create `ApplicationContext` wrapper
4. Handle L7Plugin priority dispatch
5. Remove duplicated code

**Before**:
```rust
// application/flow.rs (~140 lines)
pub async fn execute_l7(
    step: &ProcessingStep,
    container: &mut Container,
    parent_path: String,
) -> Result<TerminatorResult> {
    // 140 lines of duplicated logic with L7Plugin support
}
```

**After**:
```rust
// application/flow.rs (~5 lines)
use crate::modules::flow::engine;

pub async fn execute_l7(
    step: &ProcessingStep,
    container: &mut Container,
    parent_path: String,
) -> Result<TerminatorResult> {
    engine::execute_l7(step, container, parent_path).await
}
```

**Success Criteria**:
- L7 flow tests pass
- WebSocket upgrade still works
- HTTP requests handled correctly
- Reduced from ~140 lines to ~5 lines

---

### Phase 6: Cleanup and Documentation (1-2 hours)

**Tasks**:
1. Update `src/modules/mod.rs` to export `flow` module
2. Add module documentation
3. Run `cargo fmt`
4. Run full test suite
5. Update architecture documentation

**Files Modified**:
- `src/modules/mod.rs` - Add `pub mod flow;`
- `src/modules/flow/mod.rs` - Add comprehensive module docs
- `src/modules/kv/plugin_output.rs` - Remove moved functions (or re-export from flow)

**Success Criteria**:
- All tests pass
- No clippy warnings
- Clean module structure

---

## Work Estimate

| Phase | Time | Risk |
|-------|------|------|
| 1. Create flow module | 2-3 hours | Low |
| 2. Implement engine | 3-4 hours | Medium |
| 3. Migrate L4 | 2-3 hours | Low |
| 4. Migrate L4+ | 2-3 hours | Low |
| 5. Migrate L7 | 3-4 hours | Medium |
| 6. Cleanup & docs | 1-2 hours | Low |
| **Total** | **13-19 hours** | **Low-Medium** |

---

## Benefits

1. **Code Reduction**: ~600 lines → ~100 lines (80% reduction)
2. **Single Source of Truth**: Bug fixes apply to all layers
3. **Easier Testing**: Independent flow engine module
4. **Foundation for Plugin Refactoring**: Clean interface for Task 0.2.2
5. **Clear Separation**: Flow logic vs. layer-specific concerns

---

## Risks and Mitigation

| Risk | Mitigation |
|------|------------|
| Trait abstraction too complex | Keep ExecutionContext simple (3 methods only) |
| Performance overhead | Trait calls are zero-cost in Rust |
| L7 special cases break abstraction | Provide dedicated `execute_l7()` variant |
| Tests fail after migration | Migrate one layer at a time, test each phase |

---

## Success Criteria

- [x] Flow engine extracted to `src/modules/flow/`
- [x] L4 transport uses unified engine
- [x] L4+ carrier uses unified engine
- [x] L7 application uses unified engine
- [x] All integration tests pass
- [x] WebSocket functionality preserved
- [x] Code duplication eliminated
- [x] `cargo check` passes
- [x] `cargo fmt` applied
- [x] No clippy warnings

---

## Dependencies and Order

**Must Complete Before**:
- None (can start immediately)

**Should Complete Before**:
- Task 0.2.2 (Plugin refactoring) - Flow engine provides clean foundation
- Task 0.4 (Legacy config extraction) - Easier after flow is separated

**Related Tasks**:
- Task 1.3 (Hot-reload extraction) - May share some patterns

---

## Notes for Implementation

1. **Preserve Legacy Config**: Do not touch `dispatch_legacy_tcp()` in dispatcher
2. **Test After Each Phase**: Run `cargo check` and relevant tests
3. **L7 Special Care**: Template hijacking and L7Plugin priority must work
4. **Parent Path Tracking**: L4+ needs to pass parent_path correctly
5. **Connection Object**: Handle Virtual vs. Tcp/Udp/Tls variants properly
