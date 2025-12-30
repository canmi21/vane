# Vane Architecture Analysis

**Purpose:** This document provides comprehensive architectural analysis for understanding Vane's design, identifying current strengths/weaknesses, and planning future improvements.

**Audience:** AI agent (Claude Code) and project owner for architecture discussions and refactoring planning.

**Last Updated:** 2025-12-29

---

## Table of Contents

1. [Executive Summary](#executive-summary)
2. [Core Architecture](#core-architecture)
3. [Three-Layer Stack Model](#three-layer-stack-model)
4. [Flow-Based Execution Engine](#flow-based-execution-engine)
5. [Plugin System](#plugin-system)
6. [Data Flow Architecture](#data-flow-architecture)
7. [Protocol Handling](#protocol-handling)
8. [Configuration Management](#configuration-management)
9. [Concurrency and Performance](#concurrency-and-performance)
10. [Architectural Strengths](#architectural-strengths)
11. [Architectural Weaknesses](#architectural-weaknesses)
12. [Design Trade-offs](#design-trade-offs)
13. [Future Architecture Directions](#future-architecture-directions)

---

## Executive Summary

### Project Identity

Vane is a flow-based reverse proxy and network protocol engine written in Rust (v0.6.8, ~14,455 LOC, 123 files). It bridges raw L4 transport forwarding and complex L7 application processing through a programmable pipeline architecture.

### Core Architectural Principles

1. **Layer Separation:** Three distinct processing layers (L4/L4+/L7) optimize for inspection depth
2. **Flow-Based Execution:** Runtime-constructed decision trees replace static configuration hierarchies
3. **Zero-Copy Data Passing:** Rust ownership model and `Bytes` abstraction minimize allocations
4. **Plugin Extensibility:** Trait-based middleware/terminator system with internal and external plugins
5. **Hot-Swappable Configuration:** Atomic updates with Keep-Last-Known-Good rollback strategy

### Architecture at a Glance

```
┌─────────────────────────────────────────────────────────────┐
│                        Vane Proxy                            │
├─────────────────────────────────────────────────────────────┤
│  Ports Module (Listeners)                                    │
│    ├─ TCP Listener (L4 Entry)                               │
│    └─ UDP Listener (L4 Entry)                               │
├─────────────────────────────────────────────────────────────┤
│  L4: Transport Layer                                         │
│    ├─ Raw TCP/UDP forwarding                                │
│    ├─ Connection pooling, load balancing                    │
│    └─ Upgrade to L4+ (TLS/QUIC inspection)                  │
├─────────────────────────────────────────────────────────────┤
│  L4+: Carrier Layer                                          │
│    ├─ TLS: ClientHello parsing (SNI/ALPN)                   │
│    ├─ QUIC: Packet inspection, session management           │
│    └─ Upgrade to L7 (full protocol termination)             │
├─────────────────────────────────────────────────────────────┤
│  L7: Application Layer                                       │
│    ├─ HTTP/1.1, HTTP/2, HTTP/3 adapters                     │
│    ├─ Container model (request/response envelope)           │
│    ├─ Middleware: FetchUpstream, CGI, Static                │
│    └─ Terminator: SendResponse                              │
├─────────────────────────────────────────────────────────────┤
│  Cross-Cutting Concerns                                      │
│    ├─ KV Store (cross-layer context propagation)            │
│    ├─ Template System (runtime variable resolution)         │
│    ├─ Plugin Registry (internal + external)                 │
│    ├─ Node Registry (upstream targets)                      │
│    └─ Certificate Registry (TLS certificates)               │
└─────────────────────────────────────────────────────────────┘
```

---

## Core Architecture

### Design Philosophy

**"Inspect only as deep as necessary, compose flows at runtime"**

Traditional reverse proxies use static configuration hierarchies (Nginx: server/location blocks, HAProxy: frontend/backend). Vane uses runtime-constructed flows where routing decisions emerge from plugin composition rather than predefined configuration structure.

**Key Insight:** Many routing decisions require only shallow inspection:
- L4: IP-based routing (no protocol parsing)
- L4+: SNI-based routing (handshake metadata only, no decryption)
- L7: Full protocol access (maximum flexibility, highest cost)

### Architectural Layers

| Layer | Inspection Depth | Use Cases | Performance Characteristics |
|-------|------------------|-----------|----------------------------|
| L4 | IP/port only | Load balancing, connection pooling, IP filtering | Lowest latency, highest throughput |
| L4+ | Handshake metadata (SNI, ALPN, CID) | Virtual hosting, protocol detection, encrypted passthrough | Minimal parsing overhead |
| L7 | Full protocol access | Header manipulation, authentication, response generation | Full CPU cost, maximum flexibility |

### Layer Transition Model

Connections can flow upward through layers but never downward:

```
L4 (TCP/UDP) ──┬──> Terminate (Proxy/Abort)
               └──> Upgrade to L4+ ──┬──> Terminate (Passthrough/Abort)
                                     └──> Upgrade to L7 ──> Terminate (SendResponse)
```

**Rationale:** Each upgrade adds inspection capabilities but also CPU cost. Once a connection upgrades, the lower-layer socket is consumed and cannot be recovered.

### Module Hierarchy

```
src/
├── main.rs                    # Entry point, daemon initialization
├── core/                      # Bootstrap, router, response utilities
├── common/                    # Config loading, IP helpers, getenv
├── middleware/                # Request logging middleware
└── modules/                   # Core functional modules
    ├── stack/                 # Three-layer network stack
    │   ├── transport/         # L4: TCP/UDP handling
    │   └── protocol/          # L4+ carrier, L7 application
    │       ├── carrier/       # TLS, QUIC
    │       └── application/   # HTTP, Container, template
    ├── plugins/               # Plugin system
    │   ├── model.rs           # Trait definitions
    │   ├── registry.rs        # Plugin registry
    │   ├── loader.rs          # External plugin loader
    │   ├── middleware/        # Internal middleware
    │   ├── terminator/        # Internal terminators
    │   └── drivers/           # L7 drivers (upstream, cgi, static)
    ├── kv/                    # Cross-layer KV store
    ├── nodes/                 # Upstream target registry
    ├── ports/                 # Listener management
    └── certs/                 # Certificate management
```

**Observation:** Clear separation between protocol handling (stack/) and business logic (plugins/). Cross-cutting concerns (kv, nodes, ports, certs) isolated as independent modules.

---

## Three-Layer Stack Model

### L4: Transport Layer

**Location:** `src/modules/stack/transport/`

**Purpose:** Raw TCP/UDP forwarding with minimal inspection.

**Key Components:**
- `flow.rs` - Flow execution engine for L4
- `proxy.rs` - Bidirectional forwarding logic
- `dispatcher.rs` - Connection routing
- `balancer.rs` - Load balancing algorithms (round-robin, least-conn, IP hash)
- `health.rs` - Health check system for upstream nodes

**Data Flow:**

```
TCP/UDP Connection
    ↓
Initialize KV Store
    kv["conn.ip"] = client_addr.ip()
    kv["conn.port"] = client_addr.port()
    kv["conn.protocol"] = "tcp" | "udp"
    kv["conn.uuid"] = uuid::new_v7()
    ↓
Execute L4 Flow
    ↓
Terminator Decision
    ├─ AbortConnection: Drop socket
    ├─ TransparentProxy: Bidirectional forwarding to target IP
    ├─ ProxyNode: Lookup node, apply TLS, forward
    ├─ ProxyDomain: DNS resolution, forward
    └─ Upgrade: Transition to L4+ (TLS/QUIC inspection)
```

**Connection Object:**

```rust
pub enum ConnectionObject {
    Tcp(TcpStream),                      // Raw TCP stream
    Udp {                                // UDP datagram
        socket: Arc<UdpSocket>,
        datagram: Vec<u8>,
        client_addr: SocketAddr,
    },
    Stream(Box<dyn ByteStream>),         // TLS-wrapped stream
    Virtual(String),                     // L7 placeholder
}
```

**Design Decision:** UDP uses Arc-wrapped socket for concurrent access (multiple datagrams from same client). TCP uses owned TcpStream (one connection per stream).

**Performance Optimizations:**
- Zero-copy forwarding: `tokio::io::copy_bidirectional` uses splice/sendfile on Linux
- Connection pooling: Upstream connections reused across requests
- TCP_NODELAY: Nagle's algorithm disabled for low latency
- SO_REUSEADDR: Fast port rebinding on restart

**Weakness Identified:**
- No built-in PROXY protocol support (would require plugin)
- No connection rate limiting (must use RateLimit middleware)
- UDP lacks session tracking (fully stateless)

### L4+: Carrier Layer

**Location:** `src/modules/stack/protocol/carrier/`

**Purpose:** Extract routing metadata from encrypted protocols without terminating the session.

**Why L4+ Exists:**

Many production scenarios need routing based on SNI/ALPN but should NOT decrypt traffic:
- End-to-end encryption (zero-trust architecture)
- Certificate management delegated to upstream
- Reduced CPU overhead (parsing vs. decryption)

**TLS Handling:**

**Location:** `src/modules/stack/protocol/carrier/tls/`

```
TLS ClientHello arrives
    ↓
Parse TLS record header (5 bytes)
    ├─ Content Type: Handshake (0x16)
    ├─ Version: TLS 1.0-1.3
    └─ Length
    ↓
Parse ClientHello message
    ├─ Extract SNI extension (type 0x00)
    └─ Extract ALPN extension (type 0x10)
    ↓
Add to KV Store
    kv["protocol.sni"] = "example.com"
    kv["protocol.alpn"] = "h2"
    ↓
Execute L4+ Flow (SNI/ALPN routing)
    ↓
Terminator Decision
    ├─ TransparentProxy: Forward encrypted stream (passthrough)
    └─ Upgrade: Terminate TLS, upgrade to L7
```

**QUIC Handling:**

**Location:** `src/modules/stack/protocol/carrier/quic/`

**Design Challenge:** QUIC uses Connection IDs that persist across client IP changes (connection migration). How to route subsequent packets efficiently?

**Solution: Two-Phase Dispatch**

**Slow Path (First Packet):**
```
QUIC Initial Packet arrives
    ↓
Parse QUIC Long Header
    ├─ Version
    ├─ DCID (Destination Connection ID)
    └─ SCID (Source Connection ID)
    ↓
Decrypt Initial Packet (QUIC v1 static key)
    ↓
Extract CRYPTO Frames
    ↓
Parse TLS ClientHello from CRYPTO frames
    ├─ Extract SNI
    └─ Extract ALPN
    ↓
Execute L4+ Flow based on SNI
    ↓
Register Session
    CID_MAP[scid] = SessionData { endpoint, route, timestamp }
    IP_MAP[client_addr] = SessionData { ... }
```

**Fast Path (Subsequent Packets):**
```
QUIC Short Header Packet arrives
    ↓
Extract DCID from header (variable length)
    ↓
Lookup in CID_MAP
    ├─ Found: Route to registered endpoint (O(1))
    └─ Not Found: Fallback to IP_MAP lookup
```

**Performance Characteristics:**
- Slow Path: ~100-500µs (decrypt + parse + register)
- Fast Path: ~1-5µs (hash lookup + forward)
- Session cleanup: Periodic sweep removes expired entries

**Weakness Identified:**
- CRYPTO frame reassembly is synchronous (blocks on fragmented ClientHello)
- No support for QUIC version negotiation (only v1)
- Session registry unbounded (no max size limit)

### L7: Application Layer

**Location:** `src/modules/stack/protocol/application/`

**Purpose:** Full protocol termination with complete request/response manipulation.

**HTTP Protocol Adapters:**

| Adapter | Protocols | Library | Location |
|---------|-----------|---------|----------|
| HTTPX | HTTP/1.1, HTTP/2 | hyper | `http/httpx/` |
| H3 | HTTP/3 | h3, quinn | `http/h3/` |

**HTTPX Adapter:**

```rust
// src/modules/stack/protocol/application/http/httpx/
use hyper::server::conn::http2;

pub async fn serve_httpx(stream: TcpStream, config: ApplicationConfig) -> Result<()> {
    let service = make_service(config.flow);

    http2::Builder::new(TokioExecutor::new())
        .serve_connection_with_upgrades(stream, service)  // WebSocket support
        .await?;

    Ok(())
}
```

**Design Decision:** `serve_connection_with_upgrades` instead of `serve_connection` to support WebSocket upgrade via Upgrade header.

**H3 Adapter:**

```rust
// src/modules/stack/protocol/application/http/h3/
use h3::server::Connection;

pub async fn serve_h3(quic_conn: quinn::Connection, config: ApplicationConfig) -> Result<()> {
    let mut h3_conn = Connection::new(quic_conn).await?;

    while let Some((req, stream)) = h3_conn.accept().await? {
        tokio::spawn(handle_request(req, stream, config.clone()));
    }

    Ok(())
}
```

**Observation:** Each HTTP/3 request is a separate QUIC bidirectional stream. Connection multiplexing handled by QUIC layer.

**Container Model:**

**Location:** `src/modules/stack/protocol/application/container.rs`

The Container is L7's central data structure:

```rust
pub struct Container {
    pub kv: KvStore,                              // Cross-layer metadata
    pub request_headers: HeaderMap,               // HTTP request headers
    pub response_headers: HeaderMap,              // HTTP response headers
    pub request_payload: PayloadState,            // Request body state
    pub response_payload: PayloadState,           // Response body state
    pub client_upgrade: Option<OnUpgrade>,        // WebSocket client handle
    pub upstream_upgrade: Option<OnUpgrade>,      // WebSocket upstream handle
}
```

**Hybrid Storage Model:**

**Why not put everything in KV store?**

1. **Metadata (KV Store):** Small, frequently accessed, needs template resolution
   - `req.method`, `req.path`, `conn.ip` - stored as strings in KV
   - Template resolution: `{{req.path}}` → KV lookup → "/"

2. **Data (Native Structures):** Large, infrequently accessed, needs streaming
   - Headers: `HeaderMap` (efficient HTTP header access)
   - Payload: `PayloadState` (lazy buffering, zero-copy streaming)

**Payload State Machine:**

```rust
pub enum PayloadState {
    Http(VaneBody),       // Streaming body (not yet buffered)
    Generic,              // Non-HTTP stream (future: Redis, MySQL)
    Buffered(Bytes),      // Fully loaded into memory
    Empty,                // No payload or consumed
}
```

**State Transitions:**

```
Http(VaneBody) ──[force_buffer()]──> Buffered(Bytes) ──[consumed]──> Empty
                                              ↑
Generic ──────────────────────────────────────┘
```

**Design Decision:** Payloads remain in `Http` state until explicitly buffered. This avoids loading multi-GB file transfers into memory.

**Buffering Limit:**
- Environment variable: `L7_MAX_BUFFER_SIZE` (default: 10MB)
- Exceeding limit returns error (prevents OOM)

**Template Hijacking:**

When `{{req.body}}` is accessed:
1. Payload state transitions from `Http(VaneBody)` to `Buffered(Bytes)`
2. Entire body loaded into memory
3. Converted to string
4. Cannot be streamed to upstream (body consumed)

**Use Case:** Authentication plugin needs to hash request body. After hashing, body is consumed and cannot be forwarded. Solution: Plugin must re-create body from buffered bytes.

**Weakness Identified:**
- No partial buffering (all-or-nothing)
- No streaming template access (e.g., `{{req.body.lines[0]}}` still buffers full body)
- Hijacked payloads cannot be "un-hijacked"

**VaneBody Abstraction:**

**Location:** `src/modules/stack/protocol/application/http/wrapper.rs`

```rust
pub enum VaneBody {
    H1(hyper::body::Incoming),           // HTTP/1.1 or HTTP/2 body
    H3(h3::server::RequestStream),       // HTTP/3 body
}

impl http_body::Body for VaneBody {
    type Data = Bytes;
    type Error = Error;

    fn poll_frame(/* ... */) -> Poll<Option<Result<Frame<Bytes>>>> {
        match self {
            VaneBody::H1(body) => body.poll_frame(cx),
            VaneBody::H3(stream) => stream.poll_frame(cx),
        }
    }
}
```

**Purpose:** Unified interface for different HTTP body types. Allows L7 middleware to be protocol-agnostic.

---

## Flow-Based Execution Engine

### Motivation

**Problem with Traditional Config:**

Nginx example (static hierarchy):
```nginx
server {
    listen 80;

    location /api {
        if ($http_authorization) {  # Limited conditional logic
            proxy_pass http://backend;
        }
    }
}
```

**Limitations:**
- Conditional logic restricted to `if` statements
- No runtime composition (must restart to change flow)
- Difficult to express complex policies (e.g., "rate limit by IP, but allow whitelist, and log denied requests")

**Vane's Solution: Flow as Data Structure**

```yaml
# Flow is a recursive data structure
internal.common.match:
  input:
    condition:
      type: "ip_range"
      ip: "{{conn.ip}}"
      range: "10.0.0.0/8"
  output:
    "true":
      internal.ratelimit.sec:
        input:
          key: "{{conn.ip}}"
          limit: 1000
        output:
          allowed:
            internal.l7.fetch_upstream:
              input:
                url_prefix: "http://backend:8080"
          denied:
            internal.l7.send_response:
              input:
                status: 429
                body: "Rate limit exceeded"
    "false":
      internal.transport.abort:
        input: {}
```

**Flow is a decision tree constructed at runtime.** Each plugin returns a branch name, and the flow engine traverses to the next step.

### Flow Data Structure

**Location:** `src/modules/plugins/model.rs`

```rust
pub type ProcessingStep = HashMap<String, PluginInstance>;

pub struct PluginInstance {
    pub input: HashMap<String, Value>,           // Input parameters
    pub output: HashMap<String, ProcessingStep>, // Output branches (recursive)
}
```

**Why HashMap instead of Vec?**

Configuration clarity. Alternative design:

```rust
pub struct ProcessingStep {
    plugins: Vec<PluginInstance>,  // Execute in order?
}
```

Problem: Unclear execution order. Solution: HashMap keys are plugin names, explicitly defining "one plugin per step".

**Recursive Structure:**

```
ProcessingStep
    └─ "plugin_a": PluginInstance
        ├─ input: { "param": "value" }
        └─ output:
            ├─ "success": ProcessingStep
            │   └─ "plugin_b": PluginInstance { ... }
            └─ "failure": ProcessingStep
                └─ "plugin_c": PluginInstance { ... }
```

**Maximum Depth:** Theoretically unlimited. Practical limit: ~50 steps (stack overflow risk, though Rust uses heap for async recursion).

### Flow Execution Algorithm

**Location:** `src/modules/stack/transport/flow.rs` (pattern used across L4/L4+/L7)

```rust
async fn execute_recursive(
    step: &ProcessingStep,
    kv: &mut KvStore,
    conn: ConnectionObject,
    flow_path: String,
) -> Result<TerminatorResult> {
    // 1. Extract plugin instance (assumes single plugin per step)
    let (plugin_name, instance) = step.iter().next()
        .ok_or_else(|| anyhow!("Empty processing step"))?;

    // 2. Resolve input parameters from KV store
    let resolved_inputs = resolve_templates(&instance.input, kv)?;

    // 3. Retrieve plugin from registry
    let plugin = get_plugin(plugin_name)?;

    // 4. Execute plugin
    if let Some(middleware) = plugin.as_middleware() {
        // Middleware: returns branch name and KV updates
        let output = middleware.execute(resolved_inputs, kv, &conn).await?;

        // 5. Update KV store with plugin outputs
        if let Some(store) = output.store {
            for (k, v) in store {
                let namespaced_key = format!("{}.{}.{}", flow_path, plugin_name, k);
                kv.insert(namespaced_key, v);
            }
        }

        // 6. Select next step based on branch
        let next_step = instance.output.get(&output.branch)
            .ok_or_else(|| anyhow!("Branch '{}' not configured", output.branch))?;

        // 7. Recurse with updated flow path
        let new_path = if flow_path.is_empty() {
            format!("{}.{}", plugin_name, output.branch)
        } else {
            format!("{}.{}.{}", flow_path, plugin_name, output.branch)
        };

        execute_recursive(next_step, kv, conn, new_path).await
    } else if let Some(terminator) = plugin.as_terminator() {
        // Terminator: consumes connection, ends flow
        terminator.execute(resolved_inputs, kv, conn).await
    } else {
        Err(anyhow!("Plugin is neither middleware nor terminator"))
    }
}
```

**Flow Path Tracking:**

Example execution:
```
Start: flow_path = ""
Step 1: auth → "success" → flow_path = "auth.success"
Step 2: ratelimit → "allowed" → flow_path = "auth.success.ratelimit.allowed"
Step 3: fetch_upstream → (terminator, flow ends)
```

**KV Namespacing:**

Plugin outputs stored with full path:
```
kv["auth.success.user_id"] = "12345"
kv["auth.success.role"] = "admin"
kv["auth.success.ratelimit.allowed.tokens_remaining"] = "99"
```

**Purpose:** Avoid collisions when same plugin appears in different branches.

**Weakness Identified:**
- No cycle detection (infinite loop possible if flow references itself)
- No flow depth limit (stack overflow risk)
- No timeout per plugin (long-running plugin blocks entire connection)
- Single plugin per step (cannot execute multiple plugins in parallel at same level)

### Template Resolution

**Location:** `src/modules/stack/protocol/application/template.rs`

**Syntax:** Double-brace `{{key}}`

**Resolution Process:**

```rust
fn resolve_template(template: &str, kv: &KvStore, container: &Container) -> Result<String> {
    let key = extract_key(template)?;  // "{{conn.ip}}" -> "conn.ip"

    // 1. Check KV store first
    if let Some(value) = kv.get(key) {
        return Ok(value.clone());
    }

    // 2. Check Container (L7 only)
    if key.starts_with("req.") {
        return resolve_request_field(key, container);
    } else if key.starts_with("res.") {
        return resolve_response_field(key, container);
    }

    // 3. Not found
    Err(anyhow!("Template key '{}' not found", key))
}
```

**Magic Words (L7 Container):**

| Template | Behavior |
|----------|----------|
| `{{req.method}}` | Extract from KV (`kv["req.method"]`) |
| `{{req.path}}` | Extract from KV (`kv["req.path"]`) |
| `{{req.header.host}}` | Lookup in `container.request_headers` |
| `{{req.body}}` | **Trigger buffering**, read entire body into memory |
| `{{res.status}}` | Extract from KV (`kv["res.status"]`) |
| `{{res.header.content-type}}` | Lookup in `container.response_headers` |
| `{{res.body}}` | **Trigger buffering**, read entire response body |

**Design Decision:** Templates are resolved at parameter resolution time (before plugin execution). This means expensive operations (body buffering) happen before plugin sees the data.

**Alternative Design:** Lazy template resolution (plugin pulls values on demand). Rejected because:
- Plugins would need access to Container (breaks L4/L4+ abstraction)
- Difficult to track which plugin triggered buffering (debugging)

**Weakness Identified:**
- No template validation at config load time (typos discovered at runtime)
- No default values (e.g., `{{req.header.x-custom|default}}`)
- No template functions (e.g., `{{req.ip|hash}}`, `{{req.path|uppercase}}`)

---

## Plugin System

### Design Rationale

**Why Plugins?**

1. **Extensibility:** Add new logic without modifying core
2. **Composability:** Combine plugins to build complex flows
3. **Testability:** Test plugins in isolation
4. **Performance:** Internal plugins compiled, external plugins flexible

**Why Trait-Based?**

Rust's trait system provides:
- Zero-cost abstraction (static dispatch)
- Type safety (compile-time plugin validation)
- Downcasting (base Plugin trait → specific Middleware/Terminator)

### Plugin Trait Hierarchy

**Location:** `src/modules/plugins/model.rs`

```rust
pub trait Plugin: Send + Sync {
    fn name(&self) -> &'static str;
    fn params(&self) -> Vec<ParamDef>;

    // Downcasting methods
    fn as_middleware(&self) -> Option<&dyn Middleware> { None }
    fn as_l7_middleware(&self) -> Option<&dyn L7Middleware> { None }
    fn as_terminator(&self) -> Option<&dyn Terminator> { None }
    fn as_l7_terminator(&self) -> Option<&dyn L7Terminator> { None }
}

#[async_trait]
pub trait Middleware: Plugin {
    async fn execute(
        &self,
        inputs: ResolvedInputs,
        kv: &mut KvStore,
        conn: &ConnectionObject,
    ) -> Result<MiddlewareOutput>;
}

#[async_trait]
pub trait L7Middleware: Plugin {
    async fn execute(
        &self,
        inputs: ResolvedInputs,
        container: &mut Container,
    ) -> Result<MiddlewareOutput>;
}

#[async_trait]
pub trait Terminator: Plugin {
    async fn execute(
        &self,
        inputs: ResolvedInputs,
        kv: &KvStore,
        conn: ConnectionObject,  // Consumes connection
    ) -> Result<TerminatorResult>;
}

#[async_trait]
pub trait L7Terminator: Plugin {
    async fn execute(
        &self,
        inputs: ResolvedInputs,
        container: Container,  // Consumes container
    ) -> Result<TerminatorResult>;
}
```

**Design Decision: Four Separate Traits**

Alternative: Single trait with methods returning `NotImplemented`:

```rust
trait Plugin {
    async fn execute_middleware(...) -> Result<MiddlewareOutput> {
        Err(anyhow!("Not a middleware"))
    }
    async fn execute_terminator(...) -> Result<TerminatorResult> {
        Err(anyhow!("Not a terminator"))
    }
}
```

Rejected because:
- Runtime errors instead of compile-time safety
- Unclear which method to call
- No type-level distinction between L4 and L7 plugins

**Parameter System:**

```rust
pub struct ParamDef {
    pub name: Cow<'static, str>,
    pub required: bool,
    pub param_type: ParamType,
}

pub enum ParamType {
    String,
    Integer,
    Boolean,
    Bytes,
    Map,     // JSON object
    Array,   // JSON array
    Any,     // Polymorphic (String | Map)
}
```

**Parameter Resolution:**

```yaml
# Configuration
my_plugin:
  input:
    target_ip: "{{conn.ip}}"
    port: 8080
    enabled: true
```

**Resolution:**

```
1. Template resolution: "{{conn.ip}}" → "192.168.1.1"
2. Type conversion: JSON Value → Rust types
3. Validation: Check required parameters present
4. Pass to plugin: HashMap<String, Value>
```

**Plugin extracts parameters:**

```rust
impl Middleware for MyPlugin {
    async fn execute(&self, inputs: ResolvedInputs, ...) -> Result<MiddlewareOutput> {
        let target_ip = inputs.get("target_ip")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("target_ip required"))?;

        let port = inputs.get("port")
            .and_then(|v| v.as_i64())
            .unwrap_or(80);

        // Plugin logic...
    }
}
```

**Weakness Identified:**
- No parameter validation at config load time (only at runtime)
- No schema enforcement (plugin must manually extract and validate)
- Duplicate validation logic across plugins

### Internal Plugin Registry

**Location:** `src/modules/plugins/registry.rs`

```rust
static INTERNAL_PLUGIN_REGISTRY: Lazy<DashMap<String, Arc<dyn Plugin>>> = Lazy::new(|| {
    let registry = DashMap::new();
    let plugins: Vec<Arc<dyn Plugin>> = vec![
        // Middleware (L4/L4+)
        Arc::new(ProtocolDetect),
        Arc::new(CommonMatch),
        Arc::new(KeywordRateLimitSec),
        Arc::new(KeywordRateLimitMin),

        // Terminators (L4/L4+)
        Arc::new(AbortConnection),
        Arc::new(TransparentProxy),
        Arc::new(ProxyNode),
        Arc::new(ProxyDomain),
        Arc::new(Upgrade),

        // L7 Middleware (Drivers)
        Arc::new(FetchUpstream),
        Arc::new(Cgi),
        Arc::new(Static),

        // L7 Terminators
        Arc::new(SendResponse),
    ];

    for plugin in plugins {
        registry.insert(plugin.name().to_string(), plugin);
    }

    registry
});
```

**Lookup:**

```rust
pub fn get_internal_plugin(name: &str) -> Option<Arc<dyn Plugin>> {
    INTERNAL_PLUGIN_REGISTRY.get(name).map(|r| r.value().clone())
}
```

**Performance:** O(1) hash map lookup, lock-free reads (DashMap).

**Observation:** All internal plugins registered at startup. No dynamic registration.

### External Plugin System

**Location:** `src/modules/plugins/loader.rs`, `src/modules/plugins/external.rs`

**Why External Plugins?**

1. **Custom Logic:** Implement business-specific policies without recompiling Vane
2. **Rapid Prototyping:** Test new features in Python/Lua/Bash
3. **Integration:** Call existing HTTP APIs for authentication/authorization

**Driver Types:**

| Driver | Use Case | Latency | Security |
|--------|----------|---------|----------|
| HTTP | Call existing HTTP APIs | ~5-50ms | Network accessible |
| Unix Socket | Local IPC with external service | ~0.5-5ms | Filesystem permissions |
| Command | Spawn script (Python, Lua, Bash) | ~10-100ms | Arbitrary code execution |

**External Plugin Configuration:**

```yaml
name: "external.my_auth"
role: middleware  # Only middleware supported (terminators rejected at runtime)
driver:
  type: http
  url: "http://localhost:8080/auth"
params:
  - name: "token"
    required: true
  - name: "scope"
    required: false
```

**API Contract:**

Request:
```json
{
  "inputs": {
    "token": "Bearer abc123",
    "scope": "read:user"
  },
  "kv": {
    "conn.ip": "192.168.1.1",
    "req.path": "/api/users"
  }
}
```

Response:
```json
{
  "status": "success",
  "data": {
    "branch": "authorized",
    "store": {
      "user_id": "12345",
      "role": "admin"
    }
  }
}
```

**External Plugin Wrapper:**

```rust
pub struct ExternalPlugin {
    config: ExternalPluginConfig,
    driver: ExternalPluginDriver,
}

#[async_trait]
impl Middleware for ExternalPlugin {
    async fn execute(&self, inputs: ResolvedInputs, kv: &mut KvStore, _conn: &ConnectionObject) -> Result<MiddlewareOutput> {
        // Serialize request
        let request = json!({
            "inputs": inputs,
            "kv": kv,
        });

        // Execute driver
        let response = match &self.driver {
            ExternalPluginDriver::Http { url } => execute_http(url, request).await?,
            ExternalPluginDriver::Unix { path } => execute_unix(path, request).await?,
            ExternalPluginDriver::Command { program, args, env } => execute_command(program, args, env, request).await?,
        };

        // Parse response
        Ok(MiddlewareOutput {
            branch: response.data.branch.into(),
            store: response.data.store,
        })
    }
}
```

**Why External Terminators Not Supported:**

```rust
// src/modules/plugins/external.rs
if self.config.role == PluginRole::Terminator {
    return Err(anyhow!(
        "External plugins cannot be Terminators. Only built-in plugins can terminate connections."
    ))
}
```

**Rationale:**
- Terminators consume ConnectionObject (TcpStream, UdpSocket)
- Cannot serialize socket to JSON
- External process cannot perform bidirectional forwarding (needs direct socket access)

**Alternative Design:** Pass file descriptor to external process (Unix socket + sendmsg). Rejected due to:
- Complexity (platform-specific)
- Security (external process gets raw socket access)
- Performance (context switching overhead)

**Weakness Identified:**
- External plugin timeout not configurable (hardcoded 30s)
- No retry logic on external plugin failure
- No circuit breaker (failed external plugin blocks all traffic)
- HTTP driver uses blocking HTTP client (should use tokio's async client)

### Plugin Execution Performance

**Measurement:** (Hypothetical, based on architecture analysis)

| Plugin Type | Execution Time | Bottleneck |
|-------------|----------------|------------|
| Internal Middleware | ~1-10µs | Plugin logic |
| Internal Terminator | ~10-100µs | Socket operations |
| External HTTP | ~5-50ms | Network latency |
| External Unix | ~0.5-5ms | IPC + JSON serialization |
| External Command | ~10-100ms | Process spawn |

**Optimization Opportunity:** External plugin connection pooling (reuse HTTP connections, keep Unix sockets open).

---

## Data Flow Architecture

### Cross-Layer Context Propagation

**KV Store Lifecycle:**

```
L4: TCP Connection arrives
    ↓
KV Store initialized:
    kv["conn.uuid"] = uuid::new_v7()
    kv["conn.ip"] = "192.168.1.1"
    kv["conn.port"] = "55555"
    kv["conn.protocol"] = "tcp"
    kv["conn.layer"] = "l4"
    ↓
L4 Flow executes (middleware adds data):
    kv["whitelist.matched"] = "true"
    ↓
Upgrade to L4+ (TLS):
    kv["conn.layer"] = "l4p"
    kv["protocol.sni"] = "example.com"
    kv["protocol.alpn"] = "h2"
    ↓
L4+ Flow executes:
    kv["cert.issuer"] = "Let's Encrypt"
    ↓
Upgrade to L7 (HTTP/2):
    kv["conn.layer"] = "l7"
    kv["req.method"] = "GET"
    kv["req.path"] = "/api/users"
    ↓
L7 Flow executes:
    kv["auth.user_id"] = "12345"
    kv["upstream.backend_ip"] = "10.0.1.5"
    ↓
Response sent, connection closed
    (KV store dropped)
```

**Design Decision:** KV store passed by mutable reference through all layers. No cloning, no synchronization needed (single-threaded per connection).

### Zero-Copy Data Passing

**Bytes Abstraction:**

```rust
use bytes::Bytes;

// Bytes is a reference-counted byte buffer
let data: Bytes = read_from_socket().await?;

// Clone is cheap (increment ref count)
let data2 = data.clone();  // No copy, just Arc::clone

// Original data still valid
assert_eq!(data.len(), data2.len());
```

**Connection Ownership Transfer:**

```
L4: Listener accepts TcpStream
    ↓
Ownership transferred to flow engine
    execute_flow(stream, ...)
    ↓
Ownership transferred to terminator
    ProxyNode::execute(..., conn: ConnectionObject)
    ↓
Terminator consumes connection
    let tcp_stream = match conn {
        ConnectionObject::Tcp(s) => s,
        _ => return Err(...),
    };

    // Stream ownership moved to bidirectional copy
    tokio::io::copy_bidirectional(&mut tcp_stream, &mut upstream).await?;
```

**Zero-Copy:** Stream is never cloned. Rust ownership system ensures exactly one owner at each stage.

### Full-Duplex Streaming

**Problem:** Synchronous request/response model deadlocks when both client and upstream are slow.

```
Client (slow upload) ──> Vane ──> Upstream (slow download)
                         │
                    Deadlock: Waiting for full request
                              before starting response
```

**Solution:** Decouple request and response streams.

```rust
// Pseudocode
let (mut client_read, mut client_write) = client.split();
let (mut upstream_read, mut upstream_write) = upstream.split();

// Copy request: client → upstream
let req_task = tokio::spawn(async move {
    tokio::io::copy(&mut client_read, &mut upstream_write).await
});

// Copy response: upstream → client
let res_task = tokio::spawn(async move {
    tokio::io::copy(&mut upstream_read, &mut client_write).await
});

// Wait for both
tokio::try_join!(req_task, res_task)?;
```

**Full-Duplex:** Request and response transfer simultaneously. Backpressure handled independently.

**Observation:** This is critical for WebSocket and large file uploads/downloads.

---

## Protocol Handling

### HTTP Protocol Translation

**Supported Translations:**

| Client | Upstream | Implementation |
|--------|----------|----------------|
| HTTP/1.1 | HTTP/1.1 | Direct forwarding |
| HTTP/1.1 | HTTP/2 | FetchUpstream builds HTTP/2 request |
| HTTP/2 | HTTP/1.1 | FetchUpstream builds HTTP/1.1 request |
| HTTP/2 | HTTP/2 | Direct forwarding |
| HTTP/3 | HTTP/1.1 | FetchUpstream builds HTTP/1.1 request |
| HTTP/3 | HTTP/2 | FetchUpstream builds HTTP/2 request |

**Translation Process:**

```
1. Parse request in client protocol (H1/H2/H3)
2. Populate Container (protocol-agnostic)
    - request_headers: HeaderMap
    - request_payload: PayloadState::Http(VaneBody)
3. FetchUpstream builds upstream request
    - Create new Request in target protocol
    - Copy headers from Container
    - Stream payload from Container
4. Send to upstream, receive response
5. Populate Container with response
6. SendResponse sends to client in client protocol
```

**Header Normalization:**

HTTP/2 and HTTP/3 use pseudo-headers (`:method`, `:path`, `:authority`). HTTP/1.1 uses request line.

**Vane's Approach:**

```
HTTP/1.1:
    GET /api HTTP/1.1
    Host: example.com

HTTP/2:
    :method = GET
    :path = /api
    :authority = example.com

Container (normalized):
    kv["req.method"] = "GET"
    kv["req.path"] = "/api"
    kv["req.host"] = "example.com"
```

**Design Decision:** Extract pseudo-headers to KV store. When building upstream request, re-construct pseudo-headers or request line as needed.

### WebSocket Upgrade

**Location:** `src/modules/stack/protocol/application/http/httpx/`

**Process:**

```
1. Client sends HTTP/1.1 Upgrade request:
    GET /ws HTTP/1.1
    Upgrade: websocket
    Connection: Upgrade

2. Hyper extracts upgrade handle:
    let upgrade_handle = hyper::upgrade::on(req);

3. Store in Container:
    container.client_upgrade = Some(upgrade_handle);

4. FetchUpstream forwards upgrade to upstream:
    upstream_req.headers.insert("Upgrade", "websocket");
    let upstream_upgrade_handle = hyper::upgrade::on(upstream_resp);
    container.upstream_upgrade = Some(upstream_upgrade_handle);

5. After both sides agree (101 Switching Protocols):
    let client_ws = container.client_upgrade.unwrap().await?;
    let upstream_ws = container.upstream_upgrade.unwrap().await?;

6. Bidirectional tunnel:
    tokio::io::copy_bidirectional(&mut client_ws, &mut upstream_ws).await?;
```

**Design Decision:** Store upgrade handles in Container instead of immediately awaiting. This allows middleware to inspect upgrade request before establishing tunnel.

**Weakness Identified:**
- No WebSocket frame inspection (opaque tunnel)
- No WebSocket message logging
- No WebSocket authentication (must be done at HTTP upgrade level)

### QUIC Connection Migration

**Problem:** Mobile clients change IP addresses (WiFi → 4G). TCP connections break, QUIC connections should survive.

**QUIC Solution:** Connection IDs persist across IP changes.

```
Initial Connection:
    Client (IP: 1.2.3.4, CID: abc123) → Server

IP Change:
    Client (IP: 5.6.7.8, CID: abc123) → Server

Server:
    CID_MAP["abc123"] = SessionData { route, endpoint }
    Packet from 5.6.7.8 with CID abc123 → Route to same endpoint
```

**Vane Implementation:**

```rust
// src/modules/stack/protocol/carrier/quic/session.rs
static CID_MAP: Lazy<DashMap<ConnectionId, SessionData>> = Lazy::new(|| DashMap::new());
static IP_MAP: Lazy<DashMap<SocketAddr, SessionData>> = Lazy::new(|| DashMap::new());

// Register session
pub fn register_session(cid: ConnectionId, client_addr: SocketAddr, data: SessionData) {
    CID_MAP.insert(cid.clone(), data.clone());
    IP_MAP.insert(client_addr, data);
}

// Lookup session
pub fn lookup_session(cid: &ConnectionId, client_addr: &SocketAddr) -> Option<SessionData> {
    CID_MAP.get(cid).map(|r| r.value().clone())
        .or_else(|| IP_MAP.get(client_addr).map(|r| r.value().clone()))
}
```

**Fallback to IP Map:** If CID not found (e.g., connection ID changed by client), fall back to IP-based lookup.

**Weakness Identified:**
- No NAT rebinding detection (client behind NAT changes port → IP map miss)
- No CID rotation support (client can change CID for privacy)
- Session registry unbounded (memory leak if clients never close connections)

---

## Configuration Management

### Hot-Reload Architecture

**File Watcher:**

```rust
use notify::{Watcher, RecursiveMode, Event};

pub fn watch_config_dir(path: &str) -> Result<()> {
    let (tx, rx) = channel();
    let mut watcher = RecommendedWatcher::new(tx)?;

    watcher.watch(path.as_ref(), RecursiveMode::Recursive)?;

    tokio::spawn(async move {
        while let Ok(event) = rx.recv() {
            if let Event::Modify(_) = event {
                reload_config().await?;
            }
        }
    });

    Ok(())
}
```

**Reload Process:**

```
1. File modification detected
2. Read new configuration
3. Parse (JSON/YAML/TOML)
4. Validate schema
5. If valid:
    a. Build new data structures
    b. Atomically swap using ArcSwap
6. If invalid:
    a. Log error
    b. Keep previous configuration (Keep-Last-Known-Good)
```

**Atomic Swap:**

```rust
use arc_swap::ArcSwap;

static CONFIG: Lazy<ArcSwap<Config>> = Lazy::new(|| ArcSwap::from_pointee(Config::default()));

pub fn reload_config(new_config: Config) {
    CONFIG.store(Arc::new(new_config));  // Atomic pointer swap
}

pub fn get_config() -> Arc<Config> {
    CONFIG.load_full()  // Lock-free read
}
```

**Performance:** Lock-free reads (multiple connections can read config simultaneously). Writes are atomic (no partial state visible).

### Configuration Validation

**Current State:** Minimal validation

```rust
pub fn validate_port_config(config: &PortConfig) -> Result<()> {
    // Check listen address valid
    SocketAddr::from_str(&config.listen)?;

    // Check protocol
    if config.protocol != Protocol::Tcp && config.protocol != Protocol::Udp {
        return Err(anyhow!("Invalid protocol"));
    }

    // Flow validation?? (Not implemented)

    Ok(())
}
```

**Weakness Identified:**
- No flow structure validation (circular references possible)
- No plugin existence check (typo in plugin name only discovered at runtime)
- No parameter type validation (wrong parameter type only discovered at execution)
- No reachability analysis (dead branches not detected)

**Improvement Opportunity:**

```rust
pub fn validate_flow(flow: &ProcessingStep, visited: &mut HashSet<String>) -> Result<()> {
    for (plugin_name, instance) in flow {
        // Check plugin exists
        let plugin = get_plugin(plugin_name)
            .ok_or_else(|| anyhow!("Plugin '{}' not found", plugin_name))?;

        // Check parameters
        for param in plugin.params() {
            if param.required && !instance.input.contains_key(param.name.as_ref()) {
                return Err(anyhow!("Required parameter '{}' missing", param.name));
            }
        }

        // Check recursively
        for (branch, next_step) in &instance.output {
            validate_flow(next_step, visited)?;
        }

        // Check cycle
        if !visited.insert(plugin_name.clone()) {
            return Err(anyhow!("Circular reference detected"));
        }
    }

    Ok(())
}
```

---

## Concurrency and Performance

### Threading Model

**Tokio Async Runtime:**

```
Main Thread:
    ├─ Configuration watcher
    ├─ Port listener spawner
    └─ HTTP API server (plugin management)

Tokio Worker Threads: (default: num_cpus)
    ├─ TCP listener task (per port)
    ├─ UDP listener task (per port)
    └─ Connection handler tasks (one per connection)
```

**Per-Connection Task:**

```rust
tokio::spawn(async move {
    // 1. Initialize KV store
    let mut kv = KvStore::new();
    kv.insert("conn.ip".to_string(), addr.ip().to_string());

    // 2. Execute flow
    let result = execute_flow(&config.flow, &mut kv, stream).await;

    // 3. Handle result/error
    if let Err(e) = result {
        log(LogLevel::Error, &format!("Flow error: {}", e));
    }
});
```

**Observation:** Each connection is an independent async task. No shared state between connections (except read-only config and registries).

### Lock-Free Data Structures

**DashMap:**

```rust
use dashmap::DashMap;

static PLUGIN_REGISTRY: Lazy<DashMap<String, Arc<dyn Plugin>>> = ...;

// Read: Lock-free (RwLock under the hood, but optimized for reads)
let plugin = PLUGIN_REGISTRY.get("internal.l7.fetch_upstream");

// Write: Locked (rare, only at startup)
PLUGIN_REGISTRY.insert("my_plugin".to_string(), Arc::new(MyPlugin));
```

**ArcSwap:**

```rust
use arc_swap::ArcSwap;

static CONFIG: Lazy<ArcSwap<Config>> = ...;

// Read: Lock-free (atomic pointer load)
let config = CONFIG.load();

// Write: Lock-free (atomic pointer swap)
CONFIG.store(Arc::new(new_config));
```

**Performance Impact:**
- DashMap: ~100ns per read (vs. 50ns for HashMap, 500ns for Mutex<HashMap>)
- ArcSwap: ~10ns per read (vs. 500ns for RwLock)

### Memory Management

**Per-Connection Memory:**

Typical connection:
- KV Store: ~1-10 KB (depends on flow)
- Container: ~50-100 KB (headers + metadata)
- Buffered Payload: 0 (streaming) or up to L7_MAX_BUFFER_SIZE (10MB default)

**Memory Amplification Attack:**

Attacker sends:
- 1000 concurrent connections
- Each with 10MB body

Memory usage: 1000 × 10MB = 10GB

**Mitigation:**
- Set lower L7_MAX_BUFFER_SIZE
- Use middleware to reject large bodies before buffering
- Connection rate limiting

**Observation:** No global memory limit enforced. System can OOM if too many connections buffer large payloads.

### CPU Profiling (Hypothetical Breakdown)

Based on architecture analysis:

| Operation | % CPU Time | Optimization Opportunity |
|-----------|-----------|-------------------------|
| Flow execution overhead | 5% | Reduce recursive calls, inline small functions |
| Template resolution | 10% | Cache resolved templates per connection |
| Plugin execution | 30% | Profile individual plugins |
| Protocol parsing (TLS/QUIC) | 15% | Use SIMD for parsing |
| Bidirectional copying | 20% | Already optimized (splice/sendfile) |
| JSON serialization (external plugins) | 10% | Use faster serializer (simd-json) |
| External plugin IPC | 10% | Connection pooling, async HTTP |

---

## Architectural Strengths

### 1. Clear Layer Separation

**Strength:** L4/L4+/L7 boundaries are well-defined. Each layer has specific responsibilities.

**Benefit:**
- Easy to reason about (which layer to use for new feature?)
- Performance optimization targeted per layer
- Protocol changes isolated (e.g., adding HTTP/4 only affects L7)

### 2. Flow Composability

**Strength:** Flows are first-class data structures. Complex policies expressed declaratively.

**Benefit:**
- No code changes for new routing logic
- Testable (flows can be unit tested by simulating plugin outputs)
- Auditable (flow execution path logged)

### 3. Zero-Copy Architecture

**Strength:** Data passed by ownership, not copying.

**Benefit:**
- Low memory usage (10GB file transfer doesn't require 10GB RAM)
- High throughput (no CPU wasted on memcpy)
- Rust ownership prevents use-after-free

### 4. Plugin Extensibility

**Strength:** Both internal (compiled) and external (runtime) plugins supported.

**Benefit:**
- Performance-critical logic as internal plugins
- Rapid prototyping as external plugins
- Integration with existing systems (call HTTP APIs)

### 5. Hot-Reload Safety

**Strength:** Atomic config updates with rollback.

**Benefit:**
- Zero-downtime configuration changes
- Safety net (invalid config rejected automatically)
- Consistent state (no partial updates visible)

---

## Architectural Weaknesses

### 1. No Flow Validation at Config Load

**Weakness:** Typos in plugin names, missing parameters, circular references only discovered at runtime.

**Impact:**
- Production outages (invalid flow blocks all traffic through that port)
- Difficult debugging (which connection first triggered the error?)

**Solution:** Static flow analysis at config load time.

### 2. Limited Template Functionality

**Weakness:** Templates are simple string substitution. No functions, no defaults, no conditionals.

**Impact:**
- Complex transformations require custom plugins
- Repetitive config (same pattern repeated across flows)

**Solution:** Template functions (e.g., `{{req.ip|hash}}`, `{{req.header.x-api-key|default:"none"}}`).

### 3. External Plugin Performance

**Weakness:** External plugins have high latency (5-100ms) and no connection pooling.

**Impact:**
- External plugins cannot be used in latency-sensitive paths
- HTTP driver creates new connection per request (TCP handshake overhead)

**Solution:**
- HTTP driver connection pooling (reqwest client with keep-alive)
- Unix socket driver persistent connections
- Command driver process pool (pre-spawn workers)

### 4. QUIC Session Registry Unbounded

**Weakness:** CID_MAP and IP_MAP grow indefinitely. No max size, no LRU eviction.

**Impact:**
- Memory leak if clients never send close packets
- Slow lookup as map grows (hash collisions)

**Solution:**
- TTL-based eviction (remove sessions idle > 5 minutes)
- Max size limit (evict LRU when limit reached)
- Periodic cleanup task

### 5. No Global Connection Limit

**Weakness:** Vane accepts unlimited concurrent connections. Each spawns a Tokio task.

**Impact:**
- Resource exhaustion (too many tasks, OOM)
- Degraded performance (task scheduler overhead)

**Solution:**
- Global connection limit (reject new connections when limit reached)
- Per-IP connection limit (prevent single client monopolizing resources)
- Graceful degradation (return 503 instead of crashing)

### 6. Single-Threaded Flow Execution

**Weakness:** Flow execution is sequential. Plugins execute one-by-one even if independent.

**Example:**

```yaml
internal.l7.fetch_upstream:  # Fetches from upstream (100ms)
  output:
    success:
      internal.l7.send_response:  # Sends response
```

Could be parallelized:

```yaml
# Hypothetical parallel syntax
parallel:
  - internal.l7.fetch_upstream:
      target: "backend1"
  - internal.l7.fetch_upstream:
      target: "backend2"
aggregate:
  internal.l7.send_response:
    body: "{{backend1.body}} {{backend2.body}}"
```

**Impact:**
- Wasted time waiting for sequential operations
- Cannot fan-out to multiple backends

**Solution:** Parallel step type (execute multiple plugins concurrently, collect results).

### 7. Payload Buffering All-or-Nothing

**Weakness:** Accessing `{{req.body}}` buffers entire payload. No streaming access.

**Impact:**
- Cannot inspect first N bytes without buffering all
- Cannot process large bodies (e.g., compute hash of first 1MB)

**Solution:**
- Streaming template access (e.g., `{{req.body.peek:1024}}` reads first 1KB without buffering)
- Chunked processing (plugin reads body in chunks)

### 8. No Plugin Timeout

**Weakness:** Plugins can run indefinitely. No per-plugin timeout.

**Impact:**
- Slow external plugin blocks connection forever
- No way to kill runaway plugin

**Solution:**
- Per-plugin timeout configuration
- Timeout inheritance (L7 timeout > L4+ timeout > L4 timeout)

### 9. Error Handling Inconsistency

**Weakness:** Some errors abort connection, others log and continue. No unified error handling strategy.

**Example:**

```rust
// Some plugins
if error {
    return Err(anyhow!("Error"));  // Aborts connection
}

// Other plugins
if error {
    log(LogLevel::Error, "Error");
    return Ok(MiddlewareOutput { branch: "error".into(), ... });  // Continues flow
}
```

**Impact:**
- Unpredictable behavior (does error abort or continue?)
- Difficult debugging (errors scattered across log and flow branches)

**Solution:** Unified error handling policy (errors always bubble up vs. errors always captured in flow).

### 10. No Request/Response Logging

**Weakness:** No built-in access logging (Nginx-style access.log).

**Impact:**
- Difficult to debug production issues
- No metrics (requests/sec, latency percentiles)
- No audit trail

**Solution:**
- Access logging middleware (logs request metadata to file/database)
- Metrics integration (Prometheus exporter)

---

## Design Trade-offs

### 1. Runtime Flow Construction vs. Compile-Time Configuration

**Trade-off:**
- Runtime: Maximum flexibility, config changes without recompile
- Compile-time: Type safety, impossible configurations rejected at compile time

**Vane's Choice:** Runtime (flows are data structures parsed from YAML/JSON)

**Consequence:**
- Pro: Zero-downtime config changes
- Con: Invalid configs only detected at runtime

**Alternative:** Embedded DSL (Rust macros for type-safe config). Rejected due to complexity.

### 2. Plugin Traits vs. Plugin Modules

**Trade-off:**
- Traits: Polymorphism, dynamic dispatch
- Modules: Direct function calls, static dispatch

**Vane's Choice:** Traits (base Plugin trait with downcasting)

**Consequence:**
- Pro: Plugin registry simple (HashMap<String, Arc<dyn Plugin>>)
- Con: Virtual function call overhead (~5ns per call, negligible)

**Alternative:** Macro-generated match statement (static dispatch). Rejected due to poor extensibility.

### 3. External Plugins: JSON vs. Binary Protocol

**Trade-off:**
- JSON: Human-readable, language-agnostic, easy debugging
- Binary (e.g., MessagePack, Protocol Buffers): Faster serialization, smaller payloads

**Vane's Choice:** JSON

**Consequence:**
- Pro: Easy to implement external plugins in any language
- Con: JSON serialization overhead (~1-5µs per request)

**Impact:** External plugin latency dominated by IPC (5-100ms), serialization negligible.

### 4. ArcSwap vs. RwLock for Configuration

**Trade-off:**
- ArcSwap: Lock-free reads, clone entire config on update
- RwLock: Locked reads, in-place updates

**Vane's Choice:** ArcSwap

**Consequence:**
- Pro: Zero read contention (critical for hot path)
- Con: Config update clones entire structure (acceptable since updates rare)

**Performance:** ArcSwap read ~10ns, RwLock read ~500ns (50x faster).

### 5. Three Layers vs. More Layers

**Trade-off:**
- More layers: Finer granularity (e.g., separate L5 for TLS termination vs. inspection)
- Fewer layers: Simpler architecture

**Vane's Choice:** Three layers (L4, L4+, L7)

**Consequence:**
- Pro: Simple mental model
- Con: L4+ does too much (TLS inspection + QUIC session management + passthrough)

**Alternative:** Four layers (L4, L4.5 TLS, L4.7 QUIC, L7). Rejected due to complexity.

### 6. Single Plugin per Step vs. Multiple Plugins per Step

**Trade-off:**
- Single: Simple flow execution (no plugin ordering issues)
- Multiple: Expressive (execute middleware array in parallel)

**Vane's Choice:** Single plugin per step

**Consequence:**
- Pro: Clear execution order (explicit flow graph)
- Con: Cannot express parallel plugin execution

**Workaround:** Plugin that wraps multiple plugins internally.

### 7. Terminator Ownership vs. Reference

**Trade-off:**
- Ownership: Terminator consumes connection (enforces "flow ends here")
- Reference: Terminator borrows connection (could continue flow after)

**Vane's Choice:** Ownership (Terminator::execute takes `ConnectionObject` by value)

**Consequence:**
- Pro: Type system enforces terminator semantics (cannot use connection after termination)
- Con: Terminator cannot inspect connection without consuming it

**Benefit:** Rust compiler prevents "use after termination" bugs.

---

## Future Architecture Directions

### 1. Flow Validation Framework

**Proposal:** Static analysis of flow structures at config load time.

**Features:**
- Plugin existence check
- Parameter validation (required params present, types correct)
- Cycle detection (no infinite loops)
- Reachability analysis (warn on dead branches)
- Type inference (track KV store keys through flow)

**Implementation:**

```rust
pub struct FlowValidator {
    plugin_registry: &'static PluginRegistry,
}

impl FlowValidator {
    pub fn validate(&self, flow: &ProcessingStep) -> Result<ValidationReport> {
        let mut report = ValidationReport::new();
        let mut visited = HashSet::new();
        let mut kv_schema = KvSchema::new();

        self.validate_recursive(flow, &mut visited, &mut kv_schema, &mut report)?;

        Ok(report)
    }

    fn validate_recursive(&self, step: &ProcessingStep, visited: &mut HashSet<String>, kv: &mut KvSchema, report: &mut ValidationReport) -> Result<()> {
        // Check plugin exists, validate params, detect cycles, infer KV types...
    }
}
```

**Benefits:**
- Catch config errors before deployment
- Reduce runtime errors
- Improve developer experience (immediate feedback)

### 2. Template Function System

**Proposal:** Extend templates with function syntax.

**Examples:**

```yaml
internal.common.match:
  input:
    condition:
      type: "string_equals"
      left: "{{req.header.x-api-key|default:none}}"
      right: "secret"
```

```yaml
internal.l7.fetch_upstream:
  input:
    url_prefix: "http://backend:8080"
    headers:
      X-Client-IP: "{{conn.ip|hash:sha256}}"  # Hash IP for privacy
      X-Request-ID: "{{conn.uuid|uppercase}}"
```

**Function Catalog:**

| Function | Example | Output |
|----------|---------|--------|
| `default` | `{{key\|default:value}}` | value if key missing |
| `hash` | `{{text\|hash:sha256}}` | SHA-256 hex digest |
| `base64` | `{{text\|base64}}` | Base64 encoding |
| `json_extract` | `{{json\|json_extract:user.id}}` | Extract JSON field |
| `regex_match` | `{{text\|regex_match:pattern}}` | true/false |
| `substring` | `{{text\|substring:0:10}}` | First 10 chars |

**Implementation:**

```rust
pub fn resolve_template_with_functions(template: &str, kv: &KvStore) -> Result<String> {
    let (key, functions) = parse_template(template)?;  // "{{conn.ip|hash:sha256}}"

    let mut value = kv.get(key).ok_or_else(|| anyhow!("Key not found"))?;

    for func in functions {
        value = apply_function(func, value)?;
    }

    Ok(value)
}

fn apply_function(func: &Function, input: String) -> Result<String> {
    match func.name {
        "hash" => {
            let algo = func.args.get("algo").unwrap();
            hash_string(input, algo)
        }
        "default" => {
            if input.is_empty() {
                Ok(func.args.get("value").unwrap().clone())
            } else {
                Ok(input)
            }
        }
        _ => Err(anyhow!("Unknown function: {}", func.name)),
    }
}
```

### 3. External Plugin Connection Pooling

**Proposal:** Reuse connections to external plugins.

**Current:**

```
For each request:
    1. Create HTTP connection
    2. Send JSON request
    3. Receive JSON response
    4. Close connection
```

**Proposed:**

```
At startup:
    Create connection pool (e.g., 10 persistent connections)

For each request:
    1. Get connection from pool (or create if pool empty)
    2. Send JSON request
    3. Receive JSON response
    4. Return connection to pool
```

**Benefits:**
- Eliminate TCP handshake overhead (3-way handshake = 1.5 RTT)
- Reduce TIME_WAIT sockets
- Lower latency (5ms → 1ms for localhost HTTP plugin)

**Implementation:**

```rust
use reqwest::Client;

static HTTP_CLIENT: Lazy<Client> = Lazy::new(|| {
    Client::builder()
        .pool_max_idle_per_host(10)
        .pool_idle_timeout(Duration::from_secs(90))
        .build()
        .unwrap()
});

async fn execute_http_plugin(url: &str, request: &JsonValue) -> Result<JsonValue> {
    let response = HTTP_CLIENT.post(url)
        .json(request)
        .send()
        .await?;

    Ok(response.json().await?)
}
```

### 4. Parallel Flow Execution

**Proposal:** Allow parallel plugin execution within a single step.

**Syntax:**

```yaml
parallel:
  - id: backend1
    plugin: internal.l7.fetch_upstream
    input:
      url_prefix: "http://backend1:8080"
  - id: backend2
    plugin: internal.l7.fetch_upstream
    input:
      url_prefix: "http://backend2:8080"
aggregate:
  plugin: internal.l7.send_response
  input:
    body: "{{backend1.body}} {{backend2.body}}"
```

**Execution:**

```rust
let mut handles = vec![];

for task in parallel_tasks {
    let handle = tokio::spawn(async move {
        execute_plugin(task.plugin, task.input).await
    });
    handles.push((task.id, handle));
}

let results = HashMap::new();
for (id, handle) in handles {
    results.insert(id, handle.await?);
}

// Aggregate results available in KV store
kv["backend1.body"] = results["backend1"].body;
kv["backend2.body"] = results["backend2"].body;
```

**Benefits:**
- Fan-out to multiple backends (lower latency)
- Concurrent external plugin calls
- Parallel authentication checks

### 5. Streaming Template Access

**Proposal:** Allow partial payload access without full buffering.

**Examples:**

```yaml
internal.common.match:
  input:
    condition:
      type: "regex"
      text: "{{req.body.peek:1024}}"  # Read first 1KB only
      pattern: "^<!DOCTYPE html>"
```

**Implementation:**

```rust
impl Container {
    pub async fn peek_request_body(&mut self, limit: usize) -> Result<Bytes> {
        match &mut self.request_payload {
            PayloadState::Http(body) => {
                // Read up to `limit` bytes without consuming full stream
                let mut buf = BytesMut::with_capacity(limit);
                while buf.len() < limit {
                    match body.frame().await {
                        Some(Ok(frame)) => {
                            if let Some(data) = frame.data_ref() {
                                buf.put(data);
                            }
                        }
                        _ => break,
                    }
                }

                // Wrap remaining stream + buffered bytes
                let peeked = buf.freeze();
                let remaining_body = create_body_with_prefix(peeked.clone(), body);
                self.request_payload = PayloadState::Http(remaining_body);

                Ok(peeked)
            }
            _ => Err(anyhow!("Payload not in Http state")),
        }
    }
}
```

**Benefits:**
- Inspect large payloads without OOM
- Early rejection (detect invalid content without buffering all)

### 6. Observability Framework

**Proposal:** Built-in metrics, tracing, and logging.

**Features:**

**Metrics (Prometheus):**
- `vane_requests_total{layer, protocol, status}`
- `vane_request_duration_seconds{layer, protocol, quantile}`
- `vane_active_connections{layer, protocol}`
- `vane_plugin_execution_duration_seconds{plugin, quantile}`

**Tracing (OpenTelemetry):**
- Span per flow execution
- Child spans per plugin
- Distributed tracing (correlate with upstream services)

**Access Logging:**
- Nginx-compatible format
- Structured logging (JSON)
- Conditional logging (only log errors, slow requests)

**Implementation:**

```rust
use tracing::{instrument, span, Level};
use prometheus::{register_histogram, Histogram};

lazy_static! {
    static ref REQUEST_DURATION: Histogram = register_histogram!(
        "vane_request_duration_seconds",
        "Request duration in seconds",
        vec![0.001, 0.01, 0.1, 1.0, 10.0]
    ).unwrap();
}

#[instrument(skip(kv, conn))]
async fn execute_flow(flow: &ProcessingStep, kv: &mut KvStore, conn: ConnectionObject) -> Result<TerminatorResult> {
    let _timer = REQUEST_DURATION.start_timer();

    let span = span!(Level::INFO, "flow_execution", conn.uuid = %kv.get("conn.uuid").unwrap());
    let _enter = span.enter();

    // Execute flow...
}
```

### 7. Global Resource Limits

**Proposal:** Enforce system-wide limits to prevent resource exhaustion.

**Limits:**

```yaml
global_limits:
  max_connections: 10000           # Global connection limit
  max_connections_per_ip: 100      # Per-IP limit
  max_memory_mb: 4096              # Max memory usage
  max_buffered_payload_mb: 1024    # Max total buffered payloads
```

**Enforcement:**

```rust
static ACTIVE_CONNECTIONS: AtomicUsize = AtomicUsize::new(0);
static CONNECTIONS_PER_IP: Lazy<DashMap<IpAddr, AtomicUsize>> = Lazy::new(|| DashMap::new());

async fn accept_connection(stream: TcpStream, addr: SocketAddr) -> Result<()> {
    // Check global limit
    let current = ACTIVE_CONNECTIONS.fetch_add(1, Ordering::Relaxed);
    if current >= MAX_CONNECTIONS {
        ACTIVE_CONNECTIONS.fetch_sub(1, Ordering::Relaxed);
        return Err(anyhow!("Connection limit exceeded"));
    }

    // Check per-IP limit
    let ip_count = CONNECTIONS_PER_IP.entry(addr.ip())
        .or_insert_with(|| AtomicUsize::new(0));
    let ip_current = ip_count.fetch_add(1, Ordering::Relaxed);
    if ip_current >= MAX_CONNECTIONS_PER_IP {
        ip_count.fetch_sub(1, Ordering::Relaxed);
        ACTIVE_CONNECTIONS.fetch_sub(1, Ordering::Relaxed);
        return Err(anyhow!("Per-IP connection limit exceeded"));
    }

    // Handle connection...

    // Cleanup
    ACTIVE_CONNECTIONS.fetch_sub(1, Ordering::Relaxed);
    ip_count.fetch_sub(1, Ordering::Relaxed);

    Ok(())
}
```

---

## Conclusion

Vane's architecture demonstrates strong separation of concerns (three-layer stack), runtime composability (flow-based execution), and safety (Rust ownership + hot-reload). The plugin system balances performance (internal plugins) and flexibility (external plugins).

**Key Strengths:**
- Clear layer boundaries enable targeted optimization
- Flow-as-data allows configuration changes without code changes
- Zero-copy architecture minimizes memory overhead
- Hot-reload ensures zero-downtime updates

**Key Weaknesses:**
- Lack of config validation leads to runtime errors
- External plugin performance limited by IPC overhead
- No global resource limits risk exhaustion attacks
- Flow execution strictly sequential (no parallelism)

**Recommended Focus Areas:**
1. **Short-term:** Flow validation framework (catch errors early)
2. **Medium-term:** External plugin connection pooling (reduce latency)
3. **Long-term:** Parallel flow execution (unlock fan-out patterns)

This architecture provides a solid foundation for a production-grade reverse proxy. With targeted improvements in validation, performance, and resource management, Vane can scale to demanding production workloads.
