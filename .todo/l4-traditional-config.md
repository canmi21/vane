# Task 0.4: L4 Traditional Configuration Strategy

**Status:** Investigated - Keep As-Is (Phase II)

**User Input:** L4 保留传统配置（上古时代的传统配置），不强制要求 Flow，后面的层（L4+, L7）不支持

**Investigation Complete:** 2025-12-30 - Found legacy config system fully implemented and working

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
