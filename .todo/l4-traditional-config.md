# Task 0.4: L4 Traditional Configuration - File Extraction and Reorganization

**Status:** Planned (Phase III - Code Organization)

**User Decision:** Keep legacy config as preserved feature, extract to dedicated location

**Investigation Complete:** 2025-12-30 - Found legacy config system fully implemented and working

**Goal:** Reorganize legacy configuration code into a dedicated module without changing functionality

## Investigation Results

### What is "Traditional Configuration"?

**Traditional Configuration** = **Priority-based Protocol Detection Rules** (Pre-Flow Era)

Located in `src/modules/stack/transport/tcp.rs` (lines 9-55):

```rust
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct LegacyTcpConfig {
    #[serde(rename = "protocols")]
    pub rules: Vec<TcpProtocolRule>,  // List of detection rules
}

#[derive(Serialize, Deserialize, Debug, Clone, Validate, PartialEq, Eq)]
pub struct TcpProtocolRule {
    pub name: String,              // Rule identifier
    pub priority: u32,             // Execution order (lower = higher priority)
    pub detect: Detect,            // Detection method
    pub session: Option<TcpSession>,
    pub destination: TcpDestination,  // Where to route
}

pub enum DetectMethod {
    Magic,      // Hex pattern matching (e.g., 0x16 for TLS)
    Prefix,     // String prefix matching
    Regex,      // Regex pattern matching
    Fallback,   // Catch-all rule
}

pub enum TcpDestination {
    Resolver { resolver: String },           // Upgrade to L4+ resolver
    Forward { forward: Forward },            // Direct L4 proxy
}
```

### Current Implementation Status

**✅ Fully Implemented and Working**

1. **Configuration Union** (`tcp.rs`, lines 69-74):
   ```rust
   #[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
   #[serde(untagged)]
   pub enum TcpConfig {
       Flow(FlowConfig),           // New flow-based system
       Legacy(LegacyTcpConfig),    // Traditional protocol detection
   }
   ```

2. **Dispatcher Logic** (`dispatcher.rs`, lines 20-115):
   ```rust
   match &*config {
       TcpConfig::Legacy(legacy_config) => {
           dispatch_legacy_tcp(socket, port, legacy_config, kv_store).await;
       }
       TcpConfig::Flow(flow_config) => {
           // Flow-based execution
       }
   }
   ```

3. **Legacy Execution** (`dispatcher.rs`, lines 117-206):
   - Peek initial TCP data
   - Sort rules by priority
   - Match rules sequentially (Magic → Prefix → Regex → Fallback)
   - Route to destination (Forward or Resolver placeholder)

### Configuration Format Comparison

**Traditional (Legacy) Format:**
```yaml
protocols:
  - name: tls-443
    priority: 10
    detect:
      method: Magic
      pattern: "0x16"  # TLS handshake
    destination:
      resolver: tls

  - name: http-plaintext
    priority: 20
    detect:
      method: Prefix
      pattern: "GET"
    destination:
      forward:
        strategy: random
        nodes:
          - 192.168.1.10:8080
          - 192.168.1.11:8080

  - name: fallback
    priority: 100
    detect:
      method: Fallback
    destination:
      forward:
        strategy: first
        nodes:
          - 127.0.0.1:9999
```

**Flow-based Format:**
```yaml
connection:
  internal.protocol.detect:
    input: {}
    output:
      tls:
        internal.ratelimit.sec:
          input:
            max_rate: 100
          output:
            "true":
              internal.terminator.upgrade:
                input:
                  protocol: tls
      http:
        internal.terminator.upgrade:
          input:
            protocol: http
```

### Key Differences

| Feature | Traditional (Legacy) | Flow-based |
|---------|---------------------|------------|
| **Structure** | Flat list of rules | Tree of plugins |
| **Routing** | Priority-based matching | Branch-based logic |
| **Extensibility** | Fixed detection methods | Unlimited plugin composition |
| **Middleware** | Not supported | Full middleware chain |
| **Upgrade Path** | Direct resolver mapping | Explicit terminator plugin |
| **KV Store** | Limited (connection metadata only) | Full cross-plugin data flow |

### Current Support Status

- ✅ **L4 (Transport)**: Supports BOTH Legacy and Flow
- ❌ **L4+ (Carrier)**: Flow only (ResolverConfig)
- ❌ **L7 (Application)**: Flow only (ApplicationConfig)

### Decision: **Keep As-Is**

**Rationale:**
1. **Already Implemented**: Legacy system works, no bugs reported
2. **Backward Compatibility**: Users may have legacy configs
3. **No Effort Required**: System coexists peacefully via enum dispatch
4. **Clear Migration Path**: Users can migrate to Flow when ready
5. **L4+ and L7**: Don't need legacy (Flow is sufficient for protocol-aware routing)

### Action Items

- [x] Investigate legacy config format (completed 2025-12-30)
- [x] Document traditional vs. flow differences
- [ ] ~~Implement legacy config support~~ (already exists, no action needed)
- [ ] ~~Add validation for legacy config~~ (already exists)
- [ ] Update documentation to explain both formats (Phase IV)
- [ ] ~~Deprecate legacy config~~ (no, keep as-is)

### Related to Flow Extraction (Task 1.2)

When extracting flow engine:
1. **Keep dispatcher.rs logic intact** - It handles both legacy and flow
2. **Extract only flow execution** - Leave `dispatch_legacy_tcp()` untouched
3. **Shared components**:
   - KV Store initialization
   - Connection object creation
   - Upgrade handling (both legacy and flow can upgrade to L4+)

### Complexity Assessment

**Current Complexity**: Low - System is already working
**Future Maintenance**: Low - Legacy path is isolated, no conflicts with flow system

---

## File Extraction and Reorganization Plan

### Goal

Move legacy configuration code to a dedicated location to:
1. Clearly separate legacy (preserved) vs. flow (active development)
2. Improve code organization
3. Make it clear that legacy is "frozen" (no future updates)
4. Maintain backward compatibility (no behavior changes)

### Current File Structure

```
src/modules/stack/transport/
├── tcp.rs              # Contains BOTH TcpConfig enum and LegacyTcpConfig
├── dispatcher.rs       # Contains BOTH dispatch_flow() and dispatch_legacy_tcp()
├── flow.rs             # Flow execution (will be extracted to modules/flow/)
└── ...
```

### Target File Structure

```
src/modules/stack/transport/
├── tcp.rs              # Only FlowConfig (legacy removed)
├── dispatcher.rs       # Only dispatch_flow() (legacy removed)
├── flow.rs             # (Will be removed after flow extraction)
├── legacy/             # NEW: Dedicated legacy directory
│   ├── mod.rs          # Public API: LegacyTcpConfig, dispatch_legacy
│   ├── config.rs       # LegacyTcpConfig, TcpProtocolRule, DetectMethod, etc.
│   └── dispatcher.rs   # dispatch_legacy_tcp() function
└── ...
```

**Top-level config enum** (in `tcp.rs`):
```rust
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(untagged)]
pub enum TcpConfig {
    Flow(FlowConfig),                    // Active development
    Legacy(legacy::LegacyTcpConfig),     // Preserved, no updates
}
```

### File Reorganization Steps

#### Step 1: Create `legacy/` Directory

Create new directory: `src/modules/stack/transport/legacy/`

#### Step 2: Extract Legacy Config Definitions

Move from `tcp.rs` (lines 9-55) to `legacy/config.rs`:
- `LegacyTcpConfig`
- `TcpProtocolRule`
- `Detect` struct
- `DetectMethod` enum
- `TcpDestination` enum
- `TcpSession` struct
- All related types

#### Step 3: Extract Legacy Dispatcher

Move from `dispatcher.rs` (lines 117-206) to `legacy/dispatcher.rs`:
- `dispatch_legacy_tcp()` function
- All helper functions used by legacy dispatcher

#### Step 4: Create Legacy Module Interface

Create `legacy/mod.rs`:
```rust
/* src/modules/stack/transport/legacy/mod.rs */

//! Legacy L4 Transport Configuration System (Preserved Feature)
//!
//! This module contains the traditional priority-based protocol detection
//! configuration system that predates the flow-based architecture.
//!
//! **Status**: Preserved for backward compatibility, no future updates.
//! **Supported Layers**: L4 Transport only (L4+ and L7 do not support legacy config)
//!
//! Users should migrate to flow-based configuration for new deployments.

pub mod config;
pub mod dispatcher;

pub use config::LegacyTcpConfig;
pub use dispatcher::dispatch_legacy_tcp;
```

#### Step 5: Update Imports in Parent Files

**`tcp.rs`**:
```rust
use super::legacy::LegacyTcpConfig;  // Import from legacy module

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(untagged)]
pub enum TcpConfig {
    Flow(FlowConfig),
    Legacy(LegacyTcpConfig),  // Now from legacy::
}
```

**`dispatcher.rs`**:
```rust
use super::legacy::dispatch_legacy_tcp;

pub async fn dispatch_tcp_connection(...) {
    match &*config {
        TcpConfig::Legacy(legacy_config) => {
            dispatch_legacy_tcp(socket, port, legacy_config, kv_store).await;
        }
        TcpConfig::Flow(flow_config) => {
            // Flow path
        }
    }
}
```

#### Step 6: Update Module Declaration

**`src/modules/stack/transport/mod.rs`**:
```rust
pub mod tcp;
pub mod udp;
pub mod dispatcher;
pub mod legacy;  // NEW: Legacy config module
// ... other modules
```

### Documentation Updates

Add deprecation notice in legacy module documentation:

```rust
//! # Legacy Configuration (Preserved Feature)
//!
//! This is the traditional priority-based protocol detection system used
//! before the flow-based architecture was introduced.
//!
//! ## Status
//!
//! - **Maintained**: Yes (bug fixes only)
//! - **Active Development**: No
//! - **New Features**: No
//! - **Recommended**: Use flow-based configuration for new deployments
//!
//! ## Supported Layers
//!
//! - L4 Transport: ✅ Supported
//! - L4+ Carrier: ❌ Not supported
//! - L7 Application: ❌ Not supported
```

### Testing Strategy

After file reorganization:
1. **Compile Check**: Ensure `cargo check` passes
2. **Unit Tests**: Ensure existing tests still pass
3. **Integration Tests**: Test legacy config still works
4. **No Behavior Changes**: Functionality must remain identical

### Work Estimate

- **Complexity**: Low (file moves, no logic changes)
- **Estimated Time**: 1-2 hours
- **Risk**: Very low (pure refactoring)
- **Dependencies**: Should be done AFTER Task 1.2 (flow extraction)

### Success Criteria

- [x] Legacy config code isolated in `transport/legacy/` directory
- [x] `TcpConfig` enum still works with both variants
- [x] Dispatcher correctly routes to legacy or flow path
- [x] All tests pass
- [x] No behavior changes
- [x] Clear documentation marking legacy as preserved feature
