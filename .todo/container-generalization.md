# Task 0.2: L7 Container Generalization

**Status:** ✅ Design Decided (2025-12-29), Implementation Pending

**User Input:** L7 Container 目前是 HTTP 专用，需要通用化以支持 DNS、gRPC 等协议

## Confirmed Design: Generic Container with ProtocolData Trait

```rust
// src/modules/stack/protocol/application/container.rs
pub struct Container<P: ProtocolData> {
    pub kv: KvStore,
    pub protocol_data: P,
    pub request_payload: PayloadState,
    pub response_payload: PayloadState,
}

pub trait ProtocolData: Send + Sync {
    // Protocol-specific methods
}

// HTTP protocol
pub struct HttpData {
    pub request_headers: HeaderMap,
    pub response_headers: HeaderMap,
    pub client_upgrade: Option<OnUpgrade>,
    pub upstream_upgrade: Option<OnUpgrade>,
}

impl ProtocolData for HttpData {}

// DNS protocol (future)
pub struct DnsData {
    pub query: DnsQuery,
    pub response: Option<DnsResponse>,
}

impl ProtocolData for DnsData {}

// gRPC protocol (future)
pub struct GrpcData {
    pub metadata: MetadataMap,
    // ...
}

impl ProtocolData for GrpcData {}
```

## Design Rationale

- **Minimal code modification**: Add protocols by implementing `ProtocolData` trait, no changes to `Container` itself
- **Type-safe**: Compiler prevents protocol misuse
- **Extensible**: Community can add protocols without forking core
- **Open-closed principle**: Open for extension, closed for modification

## Confirmed Middleware Architecture: 通用 vs 细分

### 1. 通用 Middleware (General Middleware)

- **Input**: KV Store only (via template system `{{conn.ip}}`, `{{req.path}}`)
- **Processing**: Stateful or stateless logic
- **Output**: Branch name + KV updates
- **Deployment**: Can be internal or external
- **Layer**: Works at L4, L4+, L7 (protocol-agnostic)

```rust
pub trait GeneralMiddleware: Plugin {
    async fn execute(&self, kv: &mut KvStore) -> Result<MiddlewareOutput>;
}
```

**Examples**: `CommonMatch`, `RateLimit`, `GeoIP`, external auth webhook

### 2. 细分 Middleware (Protocol-Specific Middleware)

- **Input**: Full protocol context (`Container<HttpData>`, `Container<DnsData>`)
- **Processing**: Protocol-aware logic (header manipulation, payload inspection)
- **Output**: Modified protocol state + branch
- **Deployment**: Internal only (requires protocol types)
- **Layer**: Per-layer, per-protocol (e.g., L7+HTTPX, L7+DNS)

```rust
pub trait L7Middleware<P: ProtocolData>: Plugin {
    async fn execute(&self, container: &mut Container<P>) -> Result<MiddlewareOutput>;
}

// HTTP-specific
impl L7Middleware<HttpData> for FetchUpstream { ... }

// DNS-specific
impl L7Middleware<DnsData> for DnsRewrite { ... }
```

**Examples**: `FetchUpstream` (L7+HTTPX), `DnsRewrite` (L7+DNS), `GrpcLoadBalance` (L7+GRPC)

**Key Distinction: Streaming vs Buffered**
- **内置细分 middleware**: Can stream data (zero-copy, full-duplex, e.g., H2→H2 proxy, H2↔H3 bridge)
- **外置通用 middleware**: Must buffer data (read entire payload into memory)

## Confirmed: Template Hijacking Mechanism

**Purpose**: Allow external plugins to access protocol-specific data through template system

**Key Points:**
1. Each layer defines its own hijack keywords (e.g., `req.body`, `req.headers.Host`, `dns.query.name`)
2. Hijacked data **does NOT write to KV Store** (KV is for persistent cross-layer context)
3. Hijacked data is **directly passed as plugin input** for that specific plugin instance
4. External plugins receive protocol data as **buffered values** (not streams)

**Example:**
```yaml
# L7+HTTPX configuration
plugins:
  step1:
    external.webhook.auth:
      input:
        url: "https://auth.example.com"
        client_ip: "{{conn.ip}}"              # From KV (persistent)
        request_body: "{{req.body}}"          # Hijack: triggers lazy-buffer
        host_header: "{{req.headers.Host}}"  # Hijack: extracts HTTP header
      output:
        allowed: step2
        denied: abort
```

**Flow:**
1. Template engine parses `{{req.body}}`
2. Recognizes HTTPX layer hijack keyword
3. Triggers lazy-buffer: reads `Container<HttpData>.request_payload` into memory
4. Passes buffered data directly to external plugin's `request_body` parameter
5. **Does not write** `kv.set("req.body", ...)` (keeps KV clean)

**Layer-Specific Keywords:**
- **L7+HTTPX**: `req.body`, `req.headers.*`, `req.path`, `req.method`, `resp.body`, `resp.headers.*`
- **L7+DNS** (future): `dns.query.name`, `dns.query.type`, `dns.response.rcode`
- **L7+GRPC** (future): `grpc.method`, `grpc.metadata.*`, `grpc.message`

## Implementation Plan

### Phase 1: Define Trait Hierarchy
- [ ] Create `ProtocolData` trait
- [ ] Create `GeneralMiddleware` trait (KV-only)
- [ ] Create `L7Middleware<P>` trait (protocol-specific)
- [ ] Update `L7Terminator` to `L7Terminator<P>`

### Phase 2: Refactor HTTP to use Generic Container
- [ ] Rename current `Container` to `HttpData`
- [ ] Create `Container<P: ProtocolData>`
- [ ] Update all HTTP plugins to use `Container<HttpData>`
- [ ] Update L7 flow engine to support generic `Container<P>`

### Phase 3: Implement Template Hijacking
- [ ] Design hijack keyword registry (per-layer, per-protocol)
- [ ] Update template resolver to detect hijack keywords
- [ ] Trigger lazy-buffer for `{{req.body}}` etc.
- [ ] Pass hijacked values directly to plugin (skip KV)

### Phase 4: Migrate Existing Plugins
- [ ] `CommonMatch`, `RateLimit`, `ProtocolDetect` → `GeneralMiddleware`
- [ ] `FetchUpstream`, `SendResponse` → `L7Middleware<HttpData>`
- [ ] `Abort`, `ProxyNode` → `L7Terminator<HttpData>`

### Phase 5: Add New Protocols (Future)
- [ ] Implement `DnsData` + `impl ProtocolData`
- [ ] Create DNS-specific middleware (e.g., `DnsRewrite`)
- [ ] Implement `GrpcData` + `impl ProtocolData`
- [ ] Create gRPC-specific middleware (e.g., `GrpcLoadBalance`)

## Impact

Foundational change affecting all L7 plugin development

## Complexity

High (requires careful refactoring of all L7 code)

## Estimated Time

10-15 days
