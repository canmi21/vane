# Vane Architecture

This document provides a high-level architectural overview of Vane, a flow-based network protocol engine written in Rust. It explains the design principles, module organization, and execution model.

## Table of Contents

1. [Overview](#overview)
2. [Project Organization](#project-organization)
3. [Bootstrap Sequence](#bootstrap-sequence)
4. [Core Concepts](#core-concepts)
5. [Layer Architecture](#layer-architecture)
6. [Flow Execution Engine](#flow-execution-engine)
7. [Plugin System](#plugin-system)
8. [Configuration Management](#configuration-management)
9. [Template Resolution](#template-resolution)
10. [Resource Management](#resource-management)
11. [Concurrency Model](#concurrency-model)
12. [Advanced Features](#advanced-features)

## Overview

Vane is a network protocol engine that operates across three distinct layers:

- **Layer 4 (L4)**: Transport layer for raw TCP/UDP routing and forwarding
- **Layer 4+ (L4+)**: Protocol inspection layer for encrypted protocols (TLS, QUIC, HTTP)
- **Layer 7 (L7)**: Application layer for full HTTP request/response processing

The architecture follows a flow-based execution model where connections traverse dynamically defined pipelines composed of middleware and terminator plugins. Configuration changes are applied at runtime without downtime through atomic swapping.

## Project Organization

The codebase is organized into the following modules under `src/`:

```text
src/
├── bootstrap/          # System initialization and startup sequence
├── common/             # Shared utilities (config, network, system)
├── engine/             # Flow execution engine and plugin interfaces
├── ingress/            # TCP/UDP listeners and connection acceptance
├── layers/             # Layer-specific protocol implementations
│   ├── l4/             # Transport layer (TCP/UDP routing)
│   ├── l4p/            # Carrier layer (TLS/QUIC/HTTP inspection)
│   └── l7/             # Application layer (HTTP processing)
├── plugins/            # Plugin implementations (internal and external)
├── resources/          # Shared resources (KV store, certs, templates)
├── api/                # Management API
└── main.rs             # Application entry point
```

### Module Responsibilities

#### bootstrap/

Handles system initialization in a defined sequence. Key components:

- `startup.rs`: Orchestrates the 13-step bootstrap process
- `logging.rs`: Configures logging based on LOG_LEVEL environment variable
- `console.rs`: Starts the management API server
- `monitor.rs`: Manages L7 adaptive memory limits
- `socket.rs`: Creates Unix domain sockets for IPC

#### common/

Provides shared utilities across modules:

- `config/env_loader.rs`: Environment variable loading with defaults
- `config/file_loader.rs`: Configuration file reading from CONFIG_DIR
- `net/`: Network utilities (IP validation, port management)
- `sys/lifecycle.rs`: System lifecycle management
- `sys/watcher.rs`: File change detection for hot-reloading

#### engine/

Core flow execution engine:

- `executor.rs`: Recursive flow execution with timeout and circuit breaker
- `interfaces.rs`: Plugin trait definitions and data structures
- `context.rs`: Execution context abstraction (Transport vs Application)
- `key_scoping.rs`: KV key namespacing for plugin outputs

#### ingress/

Connection entry points:

- `tcp.rs`: TCP listener and connection handler
- `udp.rs`: UDP listener with datagram processing
- `listener.rs`: Unified listener management
- `state.rs`: Global configuration state (ArcSwap)
- `hotswap.rs`: Configuration reload handler
- `tasks.rs`: Connection rate limiting

#### layers/l4/

Transport layer routing:

- `dispatcher.rs`: Routes connections to flow configurations
- `flow.rs`: L4 flow execution entry point
- `resolver.rs`: Target resolution (IP, Domain, Node)
- `balancer.rs`: Load balancing strategies
- `session.rs`: UDP session state management
- `health.rs`: Health check support
- `context.rs`: Connection context and peek buffer analysis
- `proxy/`: TCP/UDP forwarding implementations

#### layers/l4p/

Protocol inspection layer:

- `plain.rs`: Plaintext HTTP protocol handler
- `tls.rs`: TLS ClientHello parsing and SNI extraction
- `quic/`: QUIC protocol support (session, muxer, protocol)
- `flow.rs`: L4+ flow execution
- `model.rs`: Protocol registry and configuration
- `hotswap.rs`: Protocol configuration reloading

#### layers/l7/

Application layer processing:

- `container.rs`: Universal L7 message envelope (request/response)
- `flow.rs`: L7 flow execution
- `protocol_data.rs`: Protocol-specific extensions
- `http/`: HTTP protocol implementations
- `model.rs`: Application registry

#### plugins/

Plugin implementations:

- `core/`: Plugin registry and loader
- `system/`: System plugins (exec, httpx, unix)
- `middleware/`: Middleware plugins (ratelimit, etc.)
- `l4/`: L4 plugins (proxy, detect)
- `l7/`: L7 plugins (upstream, cgi, static_files)

#### resources/

Shared resources:

- `kv.rs`: Per-connection key-value store (AHashMap)
- `service_discovery/`: Node and upstream management
- `certs/`: TLS certificate loading and storage
- `templates/`: Template resolution system

#### api/

Management API:

- `handlers/`: API endpoint handlers
- `middleware/auth.rs`: Token-based authentication

## Bootstrap Sequence

The system initializes through a 13-step sequence defined in `src/bootstrap/startup.rs`:

1. **Crypto Setup**: Initialize TLS backends (aws-lc-rs or ring based on feature flags)
2. **Environment Loading**: Load `.env` file using dotenvy
3. **Logging Initialization**: Configure logging level from LOG_LEVEL
4. **Infrastructure Readiness**: Ensure configuration files exist
5. **Service Discovery Load**: Load nodes.json for upstream targets
6. **Certificate Load**: Load TLS certificates from certs.json
7. **Port Configuration Load**: Load TCP/UDP listener definitions from ports.json
8. **L4+ Resolver Load**: Load protocol handlers from resolvers/ directory
9. **L7 Application Load**: Load application handlers from applications/ directory
10. **Background Tasks Start**: Start maintenance routines
11. **Plugin Initialization**: Load external plugins from plugins.json
12. **Memory Monitor Start**: Initialize L7 buffer limit monitoring
13. **Listener Activation**: Bind and listen on configured ports
14. **Hotswap Activation**: Start file watchers for configuration changes
15. **Console API Start**: Start management HTTP server
16. **Signal Wait**: Run until SIGTERM or Ctrl+C received

The sequence ensures dependencies are satisfied before dependent components initialize. Errors during critical steps halt the process.

## Core Concepts

### Flow-Based Execution

Vane uses a flow-based execution model where connections traverse pipelines defined at runtime through configuration. A flow is represented as a tree of plugin instances:

```rust
pub type ProcessingStep = HashMap<String, PluginInstance>;

pub struct PluginInstance {
    pub input: HashMap<String, Value>,
    pub output: HashMap<String, ProcessingStep>,
}
```

Each step contains exactly one plugin. Plugin inputs use template strings for variable substitution. Plugin outputs define branches that map to subsequent steps.

Example flow structure:

```json
{
  "detect_tls": {
    "input": {"buffer": "{{req.peek_buffer_hex}}"},
    "output": {
      "tls": {
        "route_by_sni": {
          "input": {"sni": "{{tls.sni}}"},
          "output": {
            "match": {"proxy": {...}},
            "no_match": {"deny": {...}}
          }
        }
      },
      "plain": {"forward": {...}}
    }
  }
}
```

### KV Store

Each connection maintains a per-connection key-value store (`KvStore`) that accumulates metadata as the connection traverses layers:

**Initial Population** (on connection accept):

- `conn.uuid`: UUIDv7 connection identifier
- `conn.ip`: Client IP address
- `conn.port`: Client port number
- `conn.proto`: Protocol type (tcp or udp)
- `conn.timestamp`: Connection establishment time
- `conn.layer`: Current processing layer (l4, l4p, or l7)

**Layer 4+ Additions**:

- `tls.sni`: TLS Server Name Indication
- `tls.alpn`: TLS Application-Layer Protocol Negotiation
- `tls.error`: TLS parsing error code (if any)
- `http.method`: HTTP request method
- `http.host`: HTTP Host header value
- `http.path`: HTTP request path
- `http.version`: HTTP version

**Plugin Output Scoping**:
Plugins can add values to the KV store. Keys are automatically scoped based on the flow path to prevent collisions:

```
plugin.<flow_path>.<plugin_name>.<key>
```

Example: A plugin named `auth` in branch `detect.tls.route` storing `username` becomes:

```
plugin.detect.tls.route.auth.username
```

### Connection Object

Connections are abstracted through the `ConnectionObject` enum to support different transport types:

```rust
pub enum ConnectionObject {
    Tcp(TcpStream),
    Udp { socket: Arc<UdpSocket>, datagram: Bytes, client_addr: SocketAddr },
    Stream(Box<dyn ByteStream>),
    Virtual(String),
}
```

- **Tcp**: Raw TCP stream for L4 operations
- **Udp**: UDP socket with datagram and client address
- **Stream**: Generic byte stream for L4+ after protocol upgrade
- **Virtual**: Placeholder for L7 where HTTP adapter manages I/O

### Layer Model

Each layer has specific responsibilities and constraints:

**Layer 4 (Transport)**:

- Operates on raw TCP streams or UDP datagrams
- Limited to transport-layer metadata (IP, port, protocol)
- Can peek initial bytes for protocol detection
- Cannot parse application protocols
- Output: Proxy to target, Deny connection, or Upgrade to L4+

**Layer 4+ (Carrier)**:

- Inspects encrypted protocols without full termination
- Parses ClientHello for TLS, QUIC headers for routing
- Extracts routing metadata (SNI, ALPN, Host header)
- Injects protocol-specific keys into KV store
- Output: Proxy with metadata, or Upgrade to L7

**Layer 7 (Application)**:

- Full HTTP request/response processing
- Access to headers, body, and protocol data
- Can modify requests before upstream forwarding
- Can generate synthetic responses
- Output: Response sent to client

## Layer Architecture

### Layer 4: Transport

Located in `src/layers/l4/`, this layer handles transport-level routing for TCP and UDP connections.

#### Entry Point

Connections enter through `dispatcher.rs` which:

1. Retrieves port configuration from `CONFIG_STATE`
2. Creates initial `KvStore` with connection metadata
3. Selects TCP or UDP configuration based on protocol
4. Delegates to `flow.rs` for execution

#### Flow Execution

The `flow.rs` module executes L4 flows:

1. Creates `TransportContext` with KV store and payload cache
2. Calls `engine::executor::execute()` with flow definition
3. Handles terminator results:
   - **Finished**: Connection completes or denied
   - **Upgrade**: Transition to L4+ protocol handler

#### Context and Peek Buffer

L4 operates on a peek buffer to make routing decisions without consuming data:

- TCP: Reads initial bytes into buffer using `peek()` system call
- UDP: Entire datagram available immediately
- Buffer size configurable: `TCP_DETECT_LIMIT` and `UDP_DETECT_LIMIT`

The peek buffer is exposed to plugins via template variable `{{req.peek_buffer_hex}}`.

#### Target Resolution

The `resolver.rs` module supports three target types:

```rust
pub enum Target {
    Ip { ip: String, port: u16 },
    Domain { domain: String, port: u16 },
    Node { node: String, port: u16 },
}
```

**IP Target**: Direct connection to IP address and port.

**Domain Target**: DNS resolution performed at connection time. Uses custom DNS resolver configured via NAMESERVER1/NAMESERVER2 environment variables. Supports A and AAAA records.

**Node Target**: Lookup in service discovery registry (`nodes.json`). Node entries contain:

- `id`: Node identifier
- `ip`: IP address
- `port`: Port number
- `weight`: Load balancing weight (optional)
- `health`: Health status (tracked by health checker)

#### Load Balancing

The `balancer.rs` module implements selection strategies when multiple targets are available:

- **Random**: Select random healthy target (uses fastrand)
- **Serial**: Round-robin selection
- **Fastest**: TCP connection race (connects to all, uses first to succeed)

Health status is considered in all strategies. Unhealthy targets are excluded.

#### UDP Session Management

UDP is connectionless but Vane maintains stateful sessions in `session.rs`:

**Session Table**:

- Key: Client IP + Port
- Value: Associated upstream socket and buffer
- TTL: Configurable via `UDP_SESSION_TIMEOUT_SECS` (default 30s)
- Cleanup: Background task removes expired entries

**Bidirectional Forwarding**:

1. Client -> Server: Create session, forward datagram
2. Server -> Client: Lookup session, forward response
3. Session reused for subsequent datagrams from same client

#### Health Checking

The `health.rs` module monitors upstream availability:

**TCP Health Checks**:

- Periodic connection attempts to upstream
- Interval: `HEALTH_TCP_INTERVAL_SECS` (default 5s)
- Timeout: `HEALTH_TCP_CONNECT_TIMEOUT_MS` (default 2000ms)
- Marks targets healthy/unhealthy based on connection success

**UDP Health Checks**:

- Passive monitoring (no active probing)
- Marks unhealthy if no response within TTL
- Cleanup interval: `HEALTH_UDP_CLEANUP_INTERVAL_SECS` (default 5s)

### Layer 4+: Carrier

Located in `src/layers/l4p/`, this layer inspects protocols for routing without full termination.

#### Supported Protocols

**Plain HTTP** (`plain.rs`):

- Detects HTTP by parsing request line
- Extracts: Method, Host header, Path, Version
- Populates KV: `http.method`, `http.host`, `http.path`, `http.version`
- Buffer size: `HTTP_PLAIN_HEADER_BUFFER_SIZE` (default 4096 bytes)

**TLS** (`tls.rs`):

- Parses ClientHello handshake message
- Extracts Server Name Indication (SNI) from extensions
- Populates KV: `tls.sni`, `tls.alpn`
- Buffer size: `TLS_CLIENTHELLO_BUFFER_SIZE` (default 4096 bytes)
- Timeout: `TLS_HANDSHAKE_PEEK_TIMEOUT_MS` (default 500ms)
- Handles fragmented ClientHello across multiple peek attempts
- Fail mode: Fail-closed by default, configurable via `TLS_ALLOW_PARSE_FAILURE`

**QUIC** (`quic/`):

- Validates QUIC packet header (fixed bit 0x40)
- Extracts Connection ID (CID) for session tracking
- Supports long and short headers
- Session table for CID -> flow mapping
- Sticky IP mapping for stateless scenarios
- Buffer limits:
  - Long header: `QUIC_LONG_HEADER_BUFFER_SIZE` (default 4096 bytes)
  - Session buffer: `QUIC_SESSION_BUFFER_LIMIT` (default 64KB)
  - Global pending: `QUIC_GLOBAL_PENDING_BYTES_LIMIT` (default 64MB)

#### Protocol Detection

Protocol detection uses magic byte analysis and heuristics:

**TLS Detection**:

- First byte: 0x16 (Handshake)
- Validates record version
- Verifies handshake type is ClientHello

**QUIC Detection**:

- First byte fixed bit: 0x40 set
- Validates version field
- Checks Connection ID length

**HTTP Detection**:

- Matches ASCII method prefixes: GET, POST, PUT, DELETE, HEAD, OPTIONS, PATCH
- Validates request line format
- Checks for HTTP version string

#### QUIC Session Management

QUIC uses a sophisticated session system in `quic/session.rs`:

**Session Table**:

- Maps Connection ID (CID) to flow metadata
- Stores flow definition and accumulated KV data
- TTL: `QUIC_SESSION_TTL_SECS` (default 300s)
- Atomic updates using DashMap

**Sticky IP Table**:

- Fallback when CID not found in session table
- Maps Client IP to flow metadata
- TTL: `QUIC_STICKY_SESSION_TTL` (default 60s)
- Used for stateless QUIC scenarios

**Fast Path**:
UDP listener checks session tables before invoking L4 flow:

1. Extract CID from QUIC packet
2. Lookup in session table
3. If hit: Use cached flow, skip L4 processing
4. If miss: Execute L4 flow, create session entry

This optimization avoids redundant flow execution for established QUIC connections.

#### QUIC Muxer

The `quic/muxer.rs` module multiplexes QUIC packets to flow tasks:

- Virtual channel per QUIC connection
- Channel capacity: `QUIC_VIRTUAL_CHANNEL_CAPACITY` (default 1024 packets)
- Demultiplexes packets based on CID
- Enforces per-session buffer limits
- Handles connection closure on channel errors

#### Resolver Configuration

L4+ configurations are stored in `resolvers/` directory:

- `tls.json`: TLS SNI-based routing
- `http.json`: HTTP Host/Path routing
- `quic.json`: QUIC-specific routing

Each resolver is a flow definition executed after protocol inspection populates the KV store.

### Layer 7: Application

Located in `src/layers/l7/`, this layer processes application protocols with full termination.

#### Container

The `Container` struct is the universal L7 message envelope:

```rust
pub struct Container {
    pub kv: KvStore,
    pub request_headers: HeaderMap,
    pub request_body: PayloadState,
    pub response_headers: HeaderMap,
    pub response_body: PayloadState,
    pub protocol_data: ProtocolData,
    send_response_tx: Option<oneshot::Sender<Response>>,
}
```

**Components**:

- `kv`: Connection metadata from L4/L4+
- `request_headers`: HTTP request headers (hyper::HeaderMap)
- `request_body`: Lazy-buffered request body
- `response_headers`: HTTP response headers
- `response_body`: Response body payload
- `protocol_data`: Protocol extensions (WebSocket upgrade handles, etc.)
- `send_response_tx`: Channel to signal response to HTTP adapter

#### Payload State

Request and response bodies use `PayloadState` enum for lazy evaluation:

```rust
pub enum PayloadState {
    Http(Body),                    // Hyper body stream (unbuffered)
    Buffered(Bytes),               // Fully buffered in memory
    Generic(Vec<u8>),              // Generic byte vector
    Empty,                         // No body
}
```

Buffering occurs on-demand when templates access `{{req.body}}` or when middleware requests buffering. This avoids unnecessary memory allocation for requests that do not require body inspection.

#### Memory Management

L7 employs adaptive memory limits to prevent resource exhaustion:

**Global Buffer Quota**:

- Default: 512MB (`L7_GLOBAL_BUFFER_LIMIT`)
- Adaptive mode: 85% of system memory (`L7_ADAPTIVE_MEMORY_RATIO`)
- Enabled via: `L7_ADAPTIVE_MEMORY_LIMIT=true`

**Per-Request Limit**:

- Default: 10MB (`L7_MAX_BUFFER_SIZE`)
- Enforced when buffering request/response bodies
- Exceeding limit returns error to client

**BufferGuard**:
RAII pattern using `BufferGuard` struct:

1. On creation: Atomically increment global counter
2. Check if quota exceeded
3. On drop: Atomically decrement global counter

This ensures automatic cleanup and prevents memory leaks.

#### HTTP Protocol Support

**HTTP/1.1**:

- Uses hyper HTTP/1 implementation
- Keepalive support via connection pooling
- Chunked transfer encoding support

**HTTP/2**:

- Requires `h2upstream` feature flag
- Multiplexing supported
- Stream window: `UPSTREAM_H2_STREAM_WINDOW` (default 2MB)
- Connection window: `UPSTREAM_H2_CONN_WINDOW` (default 2MB)

**HTTP/3**:

- Requires `h3upstream` feature flag
- Built on QUIC transport
- Integrated with QUIC muxer

#### Application Configuration

L7 applications are defined in `applications/` directory. Each application is a flow definition that can:

- Execute middleware chains
- Transform requests/responses
- Route based on headers/path
- Generate synthetic responses
- Fetch from upstream

Application registry is atomically swappable for hot-reload.

## Flow Execution Engine

The flow execution engine in `src/engine/executor.rs` is the core of Vane's processing model.

### Execution Algorithm

The `execute()` function implements recursive flow traversal:

1. **Validation**: Verify step contains exactly one plugin
2. **Input Resolution**: Resolve template strings in plugin inputs using context
3. **Plugin Lookup**: Retrieve plugin from registry by name
4. **Circuit Breaker Check**: For external plugins, check if in quiet period
5. **Dispatch**: Attempt plugin execution in priority order:
   - `HttpMiddleware` (internal, L7 only)
   - `GenericMiddleware` (internal or external, all layers)
   - `L7Middleware` (legacy L7)
   - `Middleware` (legacy generic)
   - `L7Terminator` (L7 only)
   - `Terminator` (all layers)
6. **Error Handling**: On failure, activate circuit breaker for external plugins
7. **Output Processing**:
   - **Middleware**: Update KV store with scoped keys, follow branch to next step
   - **Terminator**: Return result (Finished or Upgrade)
8. **Timeout**: Abort execution after `FLOW_EXECUTION_TIMEOUT_SECS` (default 10s)

### Middleware Output

Middleware plugins return `MiddlewareOutput`:

```rust
pub struct MiddlewareOutput {
    pub branch: Cow<'static, str>,
    pub store: Option<HashMap<String, String>>,
}
```

- `branch`: Name of output branch to follow (must exist in plugin's `output` map)
- `store`: Optional key-value pairs to add to KV store

The executor processes this by:

1. Validating branch exists in plugin configuration
2. Storing KV updates with scoped keys
3. Recursively executing the next step defined by the branch

### Terminator Result

Terminator plugins return `TerminatorResult`:

```rust
pub enum TerminatorResult {
    Finished,
    Upgrade {
        protocol: String,
        conn: ConnectionObject,
        parent_path: String,
    },
}
```

- `Finished`: Connection processing complete
- `Upgrade`: Request protocol upgrade with new connection object and protocol name

Upgrades allow layer transitions (L4 -> L4+, L4+ -> L7) by returning the connection object wrapped in the appropriate stream abstraction.

### Circuit Breaker

External plugins are protected by a passive circuit breaker:

**Failure Detection**:

- Runtime errors during plugin execution
- Middleware returning "failure" branch

**Quiet Period**:

- Duration: `EXTERNAL_PLUGIN_QUIET_PERIOD_SECS` (default 3s)
- During quiet period: Skip plugin I/O, return synthetic failure branch
- Automatic recovery after quiet period expires

**Implementation**:

- Failure timestamps stored in `EXTERNAL_PLUGIN_FAILURES` DashMap
- Checked before each plugin invocation
- Reset on successful execution

### Context Abstraction

The engine supports multiple execution contexts via the `ExecutionContext` trait:

**TransportContext** (L4/L4+):

- KV store access
- Payload cache for peek buffers
- Template resolution using KV and hex-encoded payloads

**ApplicationContext** (L7):

- Mutable Container reference
- Full HTTP request/response access
- Template resolution with HTTP hijacking (headers, body)
- On-demand buffering for body access

Contexts implement `resolve_inputs()` to handle template substitution based on available data.

### Key Scoping

Plugin outputs are namespaced to prevent collisions. The `key_scoping.rs` module provides:

**Path Construction**:

```rust
pub fn next_path(parent: &str, plugin: &str, branch: &str) -> String {
    if parent.is_empty() {
        format!("{}.{}", plugin, branch)
    } else {
        format!("{}.{}.{}", parent, plugin, branch)
    }
}
```

**Scoped Key Formatting**:

```rust
pub fn format_scoped_key(path: &str, plugin: &str, key: &str) -> String {
    if path.is_empty() {
        format!("plugin.{}.{}", plugin, key)
    } else {
        format!("plugin.{}.{}.{}", path, plugin, key)
    }
}
```

Example flow path evolution:

```
Initial: ""
After plugin "detect" (branch "tls"): "detect.tls"
After plugin "route" (branch "match"): "detect.tls.route.match"
```

Keys stored by plugin "auth" in this path:

```
plugin.detect.tls.route.match.auth.username
plugin.detect.tls.route.match.auth.role
```

This scoping ensures plugin outputs do not collide even when the same plugin appears in different branches.

## Plugin System

The plugin system supports both internal (compiled-in) and external (dynamically loaded) plugins.

### Plugin Traits

Five main traits define plugin capabilities:

**Plugin** (Base Trait):

```rust
pub trait Plugin: Send + Sync + Any {
    fn name(&self) -> &str;
    fn params(&self) -> Vec<ParamDef>;
    fn supported_protocols(&self) -> Vec<Cow<'static, str>>;
    fn as_any(&self) -> &dyn Any;
    // Downcast methods for specific traits...
}
```

**GenericMiddleware** (Cross-Layer):

```rust
pub trait GenericMiddleware: Plugin {
    fn output(&self) -> Vec<Cow<'static, str>>;
    async fn execute(&self, inputs: ResolvedInputs) -> Result<MiddlewareOutput>;
}
```

- Used for: Protocol-agnostic middleware (rate limiting, detection, routing)
- Access: Template-resolved inputs only
- External: Supported

**HttpMiddleware** (L7 HTTP):

```rust
pub trait HttpMiddleware: Plugin {
    fn output(&self) -> Vec<Cow<'static, str>>;
    async fn execute(
        &self,
        context: &mut (dyn Any + Send),
        inputs: ResolvedInputs,
    ) -> Result<MiddlewareOutput>;
}
```

- Used for: HTTP-specific transformations (header manipulation, body inspection)
- Access: Full Container (downcast from Any)
- External: Not supported (requires Rust implementation)

**Terminator** (Generic):

```rust
pub trait Terminator: Plugin {
    fn supported_layers(&self) -> Vec<Layer>;
    async fn execute(
        &self,
        inputs: ResolvedInputs,
        kv: &mut KvStore,
        conn: ConnectionObject,
    ) -> Result<TerminatorResult>;
}
```

- Used for: Connection termination (proxy, deny, upgrade)
- Access: KV store and connection object
- External: Not currently supported

**L7Terminator** (L7 Privileged):

```rust
pub trait L7Terminator: Plugin {
    async fn execute_l7(
        &self,
        context: &mut (dyn Any + Send),
        inputs: ResolvedInputs,
    ) -> Result<TerminatorResult>;
}
```

- Used for: L7-specific terminators (send_response, fetch_upstream)
- Access: Full Container
- External: Not supported

### Plugin Registry

The registry in `src/plugins/core/registry.rs` manages plugin lifecycle:

**Internal Plugin Registration**:

```rust
pub fn register_plugin(plugin: Arc<dyn Plugin>) {
    PLUGIN_REGISTRY.insert(plugin.name().to_string(), plugin);
}
```

Internal plugins are registered at compile-time using static initialization.

**External Plugin Registration**:

```rust
pub fn register_external_plugin(
    name: String,
    plugin: Arc<dyn Plugin>,
    config: ExternalPluginConfig,
) {
    EXTERNAL_PLUGIN_REGISTRY.write().insert(name.clone(), config);
    PLUGIN_REGISTRY.insert(name, plugin);
}
```

External plugins are loaded from `plugins.json` during bootstrap.

**Plugin Lookup**:

```rust
pub fn get_plugin(name: &str) -> Option<Arc<dyn Plugin>> {
    PLUGIN_REGISTRY.get(name).map(|e| Arc::clone(e.value()))
}
```

Thread-safe using DashMap for concurrent access.

### External Plugin Drivers

External plugins use one of three driver types:

**HTTP Driver**:

```json
{
	"type": "http",
	"url": "http://localhost:9000/plugin"
}
```

- Sends POST request with JSON payload
- Expects JSON response with `status`, `data`, `message` fields
- Timeout: `FLOW_EXECUTION_TIMEOUT_SECS`
- TLS verification: Controlled by `SKIP_TLS_VERIFY`

**Unix Socket Driver**:

```json
{
	"type": "unix",
	"path": "/var/run/plugin.sock"
}
```

- Connects to Unix domain socket
- Protocol: Same as HTTP driver (JSON over socket)

**Command Driver**:

```json
{
	"type": "command",
	"program": "/usr/bin/plugin",
	"args": ["--mode", "middleware"],
	"env": { "KEY": "value" }
}
```

- Spawns process for each invocation
- Input: JSON sent to stdin
- Output: JSON read from stdout
- Environment sanitization: Controlled by security flags
- Timeout: `FLOW_EXECUTION_TIMEOUT_SECS`

### Plugin Configuration

External plugins are defined in `plugins.json`:

```json
{
	"plugins": [
		{
			"name": "custom_auth",
			"role": "middleware",
			"driver": {
				"type": "http",
				"url": "http://localhost:9001/auth"
			},
			"params": [{ "name": "token", "required": true }],
			"output": ["success", "failure"]
		}
	]
}
```

**Validation**:

- Parameter definitions checked at load time
- Output branches must be non-empty for middleware
- Role must be "middleware" or "terminator"

**Health Checking**:

- Periodic validation interval: `EXTERNAL_PLUGIN_CHECK_INTERVAL_MINS` (default 15 minutes)
- Ensures plugin processes/servers remain responsive
- Circuit breaker activates on failures

### Parameter Validation

Plugin inputs are validated against parameter definitions:

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
    Map,     // JSON Object
    Array,   // JSON Array
    Any,     // Polymorphic (String | Map)
}
```

Validation occurs before plugin execution:

1. Check all required parameters are present
2. Verify parameter types match definitions
3. Reject execution if validation fails

### Built-in Plugins

**Detection Plugins** (`plugins/l4/detect/`):

- `tls_detect`: TLS ClientHello detection
- `quic_detect`: QUIC packet detection
- `http_detect`: HTTP method detection
- `dns_detect`: DNS query detection

**Proxy Plugins** (`plugins/l4/proxy/`):

- `tcp_proxy`: TCP forwarding
- `udp_proxy`: UDP forwarding
- `deny`: Connection rejection

**Middleware Plugins** (`plugins/middleware/`):

- `ratelimit`: Rate limiting based on keys
- `header_inject`: HTTP header manipulation
- `log`: Request logging

**L7 Terminators** (`plugins/l7/`):

- `http_fetch`: Upstream HTTP request
- `send_response`: Direct response generation
- `static_serve`: Static file serving
- `cgi`: CGI script execution

## Configuration Management

Vane's configuration system supports hot-reloading without downtime.

### Configuration Files

**ports.json**:
Defines TCP/UDP listeners and their L4 flows:

```json
{
  "ports": [
    {
      "port": 443,
      "tcp": {
        "flow": {...}
      },
      "udp": {
        "flow": {...}
      }
    }
  ]
}
```

**resolvers/\*.json**:
L4+ protocol handlers:

- `tls.json`: TLS SNI routing
- `http.json`: HTTP Host/Path routing
- `quic.json`: QUIC CID routing

**applications/\*.json**:
L7 application pipelines:

- Define middleware chains
- Configure upstream targets
- Set response policies

**nodes.json**:
Service discovery registry:

```json
{
	"nodes": {
		"backend1": {
			"ip": "192.168.1.10",
			"port": 8080,
			"weight": 100
		}
	}
}
```

**certs.json**:
TLS certificate definitions:

```json
{
	"certificates": [
		{
			"domains": ["example.com", "*.example.com"],
			"cert_path": "/path/to/cert.pem",
			"key_path": "/path/to/key.pem"
		}
	]
}
```

**plugins.json**:
External plugin registry (see Plugin System section).

### File Watching

The `watcher.rs` module monitors configuration files using the notify crate:

**Watch Targets**:

- `CONFIG_DIR/ports.json`
- `CONFIG_DIR/resolvers/*.json`
- `CONFIG_DIR/applications/*.json`
- `CONFIG_DIR/nodes.json`
- `CONFIG_DIR/certs.json`
- `CONFIG_DIR/plugins.json`

**Debouncing**:
File change events are debounced (typically 100-500ms) to avoid redundant reloads when editors create temporary files.

**Notification**:
Changes are sent via Tokio channels to respective hotswap handlers.

### Atomic Swapping

Configuration updates use `arc-swap` for lock-free atomic pointer swaps:

**Port Configuration**:

```rust
pub static CONFIG_STATE: ArcSwap<Vec<PortStatus>> = ArcSwap::from_pointee(vec![]);
```

**Protocol Registry**:

```rust
pub static RESOLVER_REGISTRY: ArcSwap<HashMap<String, ProtocolFlow>> = ArcSwap::from_pointee(HashMap::new());
```

**Application Registry**:

```rust
pub static APPLICATION_REGISTRY: ArcSwap<HashMap<String, ApplicationConfig>> = ArcSwap::from_pointee(HashMap::new());
```

**Swap Process**:

1. Detect file change via watcher
2. Load and parse new configuration
3. Validate configuration syntax and semantics
4. On success: `registry.store(Arc::new(new_config))`
5. On failure: Log error, retain previous configuration

**Consistency**:

- In-flight connections use configuration snapshot from start time
- New connections immediately see updated configuration
- No connection drops or service interruption

### Validation

Configuration validation occurs before swapping:

**Syntax Validation**:

- JSON parsing errors
- Required fields present
- Type checking

**Semantic Validation**:

- Plugin existence verification
- Parameter requirement checking
- Output branch completeness
- Target reachability (optional, best-effort)

**Layer Validation**:

- L4 flows cannot use L7-specific plugins
- L7 flows cannot use L4-specific terminators
- Protocol compatibility checking

Validation failures prevent configuration swap and log detailed error messages.

## Template Resolution

The template system in `src/resources/templates/` provides variable substitution in plugin inputs.

### Template Syntax

Templates use double-brace syntax: `{{variable_name}}`

**Direct KV Lookup**:

```
{{conn.ip}}          -> "192.168.1.100"
{{tls.sni}}          -> "example.com"
{{plugin.auth.user}} -> "admin"
```

**Nested Access** (for JSON values in KV):

```
{{request.headers.host}}      -> Access nested JSON field
{{plugin.parser.data.field}}  -> Scoped plugin output access
```

### Context Hierarchy

Template resolution depends on execution context:

**L4/L4+ (TransportContext)**:

- KV store lookup
- Payload hijacking for raw data:
  - `{{req.peek_buffer_hex}}`: Hex-encoded peek buffer
  - `{{req.peek_buffer_bytes}}`: Raw bytes (if supported by plugin)

**L7 (ApplicationContext)**:

- KV store lookup
- HTTP hijacking:
  - `{{req.body}}`: Request body (triggers buffering)
  - `{{req.header.name}}`: Request header by name
  - `{{res.header.name}}`: Response header by name
  - `{{req.method}}`: HTTP method
  - `{{req.path}}`: HTTP path
  - `{{req.query.param}}`: Query parameter

### Resolution Algorithm

Implemented in `src/resources/templates/mod.rs`:

1. **Parse Template**: Extract `{{key}}` patterns using regex
2. **Classify Key**: Determine if KV lookup or hijacking required
3. **Resolve Value**:
   - KV keys: Direct lookup in KV store
   - Hijacked keys: Context-specific extraction (hex encoding, header lookup, etc.)
4. **Substitute**: Replace `{{key}}` with resolved value
5. **Repeat**: Process nested templates up to depth limit

**Depth Limit**: `MAX_TEMPLATE_DEPTH` (default 5) prevents infinite recursion.

**Size Limit**: `MAX_TEMPLATE_RESULT_SIZE` (default 64KB) prevents memory exhaustion.

### Security Considerations

**Key Validation**:
Plugin outputs are validated to prevent template injection:

- Keys containing `{` or `}` are rejected
- Prevents plugins from injecting template syntax into KV store

**Injection Prevention**:
Template resolution is read-only. Plugins cannot modify the template resolution process.

**DoS Protection**:
Depth and size limits prevent resource exhaustion attacks through deeply nested or large templates.

## Resource Management

### Connection Rate Limiting

Global connection tracking in `src/ingress/tasks.rs`:

**Per-IP Limiting**:

- Limit: `MAX_CONNECTIONS_PER_IP` (default 50)
- Tracked using DashMap: IP -> connection count
- Atomic increment on accept, decrement on close
- Exceeding limit: Connection rejected with log entry

**Global Limiting**:

- Limit: `MAX_CONNECTIONS` (default 10000)
- Atomic counter for total active connections
- Exceeding limit: New connections rejected

**Cleanup**:
Connection counts decremented automatically using RAII guards that execute on connection drop.

### Buffer Management

**TCP Buffers**:

- System socket buffers used (kernel managed)
- Peek buffer size: `TCP_DETECT_LIMIT` (default 64 bytes)

**UDP Buffers**:

- Datagram size: `UDP_DETECT_LIMIT` (default 64 bytes) for protocol detection
- Session buffers: `UDP_SESSION_BUFFER` (default 4MB) per session
- QUIC buffers: See QUIC section

**L7 Buffers**:

- Managed through BufferGuard system
- Global quota enforcement
- Per-request limits
- Automatic cleanup on drop

### Upstream Connection Pooling

HTTP upstream connections use pooling in `src/plugins/l7/upstream/pool.rs`:

**Configuration**:

- Idle timeout: `UPSTREAM_POOL_IDLE_TIMEOUT` (default 90s)
- Max idle connections: `UPSTREAM_POOL_MAX_IDLE` (default 32)
- Keepalive interval: `UPSTREAM_KEEPALIVE_INTERVAL` (default 30s)

**HTTP/2 Specific**:

- Stream window: `UPSTREAM_H2_STREAM_WINDOW` (default 2MB)
- Connection window: `UPSTREAM_H2_CONN_WINDOW` (default 2MB)

**Pool Behavior**:

- Reuses connections when possible
- Closes idle connections after timeout
- Per-host connection limits
- Thread-safe using Arc and Mutex

**QUIC Pooling** (`quic_pool.rs`):

- Separate pool for QUIC/HTTP3 connections
- Idle timeout: `UPSTREAM_POOL_IDLE_TIMEOUT` (default 90s)
- Connection reuse based on server name

### TLS Certificate Storage

Certificates loaded in `src/resources/certs/loader.rs`:

**Storage Structure**:

```rust
pub static CERT_REGISTRY: ArcSwap<HashMap<String, Arc<CertifiedKey>>> = ...;
```

**Key**: Domain name (supports wildcards)
**Value**: `CertifiedKey` containing certificate chain and private key

**Lookup**:

1. Exact domain match
2. Wildcard match (e.g., `*.example.com` matches `www.example.com`)
3. Fallback to default certificate (if configured)

**Hot-Reload**:
Certificate updates detected via file watcher, atomically swapped without dropping connections.

## Concurrency Model

### Task Model

**Per-Connection Tasks**:

- One Tokio task per TCP connection
- Task spawned on accept, runs until connection close
- Connection object owned by task (no sharing)
- KV store owned by task

**UDP Datagram Processing**:

- One Tokio task per UDP datagram
- Short-lived task (completes after forwarding)
- Session table shared across datagrams (DashMap for thread-safety)

**Background Tasks**:

- File watchers: One task per watched directory/file
- Health checkers: One task per check type (TCP/UDP)
- Memory monitor: One task for L7 buffer tracking
- Plugin health: One task for external plugin validation
- Console API: One task for HTTP server

### Shared State

**ArcSwap Registries** (Read-Heavy):

- `CONFIG_STATE`: Port configurations
- `RESOLVER_REGISTRY`: L4+ protocol handlers
- `APPLICATION_REGISTRY`: L7 application flows
- `NODES_STATE`: Service discovery nodes
- `CERT_REGISTRY`: TLS certificates

Lock-free reads, atomic pointer swaps for updates.

**DashMap Tables** (Read-Write):

- `PLUGIN_REGISTRY`: Plugin lookup
- `EXTERNAL_PLUGIN_REGISTRY`: External plugin metadata
- `EXTERNAL_PLUGIN_FAILURES`: Circuit breaker state
- QUIC session tables: CID and sticky IP mappings
- UDP session table: Client -> upstream mapping
- Connection rate limit counters: IP -> count

Concurrent read/write with internal sharding for performance.

**Atomic Counters**:

- Global connection count (AtomicUsize)
- Per-IP connection counts (stored in DashMap)
- L7 buffer quota (AtomicUsize)

### Synchronization Primitives

**Channels**:

- File watcher -> Hotswap handler: `tokio::sync::mpsc`
- HTTP adapter -> L7 flow: `tokio::sync::oneshot`
- QUIC muxer: `tokio::sync::mpsc` with bounded capacity

**RwLock** (Rare):

- External plugin registry writes (infrequent updates)

**Mutex** (Avoided):

- Connection pooling uses `hyper::client::pool` internal locks
- Generally avoided in hot path

### Async Runtime

**Tokio Multi-Threaded Runtime**:

- Worker threads: Default = CPU core count
- Work-stealing scheduler
- All I/O operations are async (TcpStream, UdpSocket)

**Blocking Operations**:

- File I/O for configuration loading: Uses `tokio::fs` (backed by thread pool)
- DNS resolution: Uses `trust-dns-resolver` async resolver

**Cancellation**:

- Connection tasks cancelled on client disconnect (Tokio task cancellation)
- Flow execution timeout enforced via `tokio::time::timeout`
- Graceful shutdown on SIGTERM (active connections allowed to complete)

## Advanced Features

### Rate Limiting

Implemented in `src/plugins/middleware/ratelimit.rs`:

**Algorithm**: Token bucket using `governor` crate

**Configuration**:

- Max memory: `MAX_LIMITER_MEMORY` (default 4MB)
- Key max length: `RATELIMIT_KEY_MAX_LEN` (default 256 bytes)

**Key Sources**:

- Client IP: `{{conn.ip}}`
- Custom keys: Template-based (e.g., `{{plugin.auth.user}}`)

**Rate Formats**:

- Per-second: "100/s"
- Per-minute: "1000/m"
- Per-hour: "10000/h"

**Behavior**:

- Returns "allowed" or "denied" branch
- Middleware plugin (can be chained)
- Thread-safe quota tracking

### CGI Support

Implemented in `src/plugins/l7/cgi/executor.rs`:

**Environment Variables**:
Standard CGI variables populated:

- `GATEWAY_INTERFACE=CGI/1.1`
- `SERVER_PROTOCOL=HTTP/1.1`
- `REQUEST_METHOD`, `REQUEST_URI`, `QUERY_STRING`
- `REMOTE_ADDR`, `REMOTE_PORT`
- `SCRIPT_FILENAME`, `SCRIPT_NAME`
- `CONTENT_LENGTH`, `CONTENT_TYPE`
- HTTP headers as `HTTP_*` variables

**Execution**:

- Spawns process using `tokio::process::Command`
- Request body sent to stdin
- Response read from stdout
- Timeout: `CGI_BODY_TIMEOUT_SEC` (default 30s)
- Max body size: `CGI_BODY_MAX_SIZE_BYTE` (default 10MB)

**Response Parsing**:

- Parses CGI response headers
- Converts to HTTP response
- Supports both parsed headers and document responses

### Static File Serving

Implemented in `src/plugins/l7/static_files/`:

**MIME Detection**:

- Uses `infer` crate for content sniffing
- Sniff bytes: `STATIC_MIME_SNIFF_BYTES` (default 512)
- Fallback to file extension mapping
- Default: `application/octet-stream`

**Features**:

- Range request support
- ETag generation
- Last-Modified headers
- Conditional GET support
- Directory listing (optional)

**Security**:

- Path traversal prevention
- Configurable root directory
- Hidden file filtering

### DNS Resolution

Implemented in `src/layers/l4/resolver.rs`:

**Resolver Configuration**:

- Primary nameserver: `NAMESERVER1` (default 1.1.1.1)
- Primary port: `NAMESERVER1_PORT` (default 53)
- Secondary nameserver: `NAMESERVER2` (default 8.8.8.8)
- Secondary port: `NAMESERVER2_PORT` (default 53)

**Implementation**:

- Uses `trust-dns-resolver` async resolver
- Supports A and AAAA records
- Concurrent queries to both nameservers
- Returns first successful response
- Caching handled by resolver library

### Protocol Upgrades

Connections can upgrade across layers:

**L4 -> L4+**:

1. L4 terminator returns `TerminatorResult::Upgrade`
2. Specifies protocol name (e.g., "tls", "quic", "http")
3. Connection object wrapped in appropriate stream
4. L4+ dispatcher invoked with protocol handler

**L4+ -> L7**:

1. L4+ terminator returns `TerminatorResult::Upgrade`
2. Specifies protocol name (e.g., "httpx", "h3")
3. HTTP adapter creates Container from request
4. L7 flow executed with Container context

**WebSocket Upgrade** (L7 internal):

- Handled via `protocol_data` in Container
- Upgrade handle stored for post-processing
- Response sent with 101 Switching Protocols
- Connection transitioned to WebSocket frame handling

### Health and Monitoring

**Logging**:

- Structured logging via `fancy_log` crate
- Log levels: trace, debug, info, warn, error
- Emoji prefixes for visual parsing
- Configurable via `LOG_LEVEL` environment variable

**Metrics** (Future):

- Planned Prometheus exporter
- Connection counts, request rates, latency percentiles
- Plugin execution times, error rates

**Management API**:

- REST API on port `PORT` (default 3333)
- Authentication via `ACCESS_TOKEN` (optional)
- Endpoints:
  - `GET /`: System information
  - `GET /health`: Health check
  - Additional endpoints for configuration inspection

### Security Features

**External Plugin Sandboxing**:

- Environment variable filtering for command plugins
- Linker env: `ALLOW_EXTERNAL_LINKER_ENV` (default false)
- Runtime env: `ALLOW_EXTERNAL_RUNTIME_ENV` (default false)
- Shell env: `ALLOW_EXTERNAL_SHELL_ENV` (default false)
- PATH append: `ALLOW_EXTERNAL_PATH_ENV_APPEND` (default false)

**Plugin Validation**:

- External plugin signature verification (if enabled)
- Skip via: `SKIP_EXTERNAL_PLUGIN_VALIDATION` (default false)

**TLS Verification**:

- Upstream TLS verification enabled by default
- Skip via: `SKIP_TLS_VERIFY` (default false, security risk)

**Input Sanitization**:

- Template injection prevention
- Parameter type validation
- Key name validation (reject braces)

**Resource Limits**:

- Connection rate limiting
- Memory quotas
- Execution timeouts
- Buffer size limits

This architecture provides a modular, extensible platform for network protocol handling with strong isolation between layers, pluggable middleware, and runtime reconfigurability.
