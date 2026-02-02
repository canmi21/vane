# Vane Architecture

> Flow-based, multi-layer reverse proxy engine in Rust. ~12k SLoC production code, ~15k SLoC tests.

## System Overview

```
                    ┌─────────────────────────────────────┐
                    │           main.rs                    │
                    │  bootstrap::startup::start()         │
                    └──────────────┬──────────────────────┘
                                   │
            ┌──────────────────────┼──────────────────────┐
            │                      │                      │
     ┌──────▼──────┐    ┌─────────▼────────┐    ┌────────▼────────┐
     │   Config     │    │    Ingress        │    │  Console (API)   │
     │  (live crate)│    │  TCP/UDP Bind     │    │  Axum REST       │
     └──────┬──────┘    └─────────┬────────┘    └─────────────────┘
            │                     │
            │              ┌──────▼──────┐
            │              │  Dispatcher  │
            │              └──────┬──────┘
            │                     │
            │     ┌───────────────┼───────────────┐
            │     │               │               │
         ┌──▼─────▼──┐    ┌──────▼──────┐  ┌─────▼──────┐
         │    L4      │    │    L4+       │  │    L7      │
         │  TCP/UDP   │    │  TLS/QUIC    │  │  HTTP/1/2/3│
         │  Forward   │    │  Inspect/    │  │  Full      │
         │            │    │  Terminate   │  │  Duplex    │
         └────────────┘    └─────────────┘  └────────────┘
              │                   │               │
              └───────────────────┼───────────────┘
                                  │
                         ┌────────▼────────┐
                         │   Flow Engine    │
                         │  (executor.rs)   │
                         │  Middleware →    │
                         │  Branch →       │
                         │  Terminator     │
                         └────────┬────────┘
                                  │
                         ┌────────▼────────┐
                         │  Plugin System   │
                         │  Internal +      │
                         │  External        │
                         └─────────────────┘
```

## Data Flow: Connection Lifecycle

```
TCP/UDP Packet
  → ingress/listener.rs          bind socket, accept connection
  → ingress/tcp.rs / udp.rs      spawn per-connection task
  → layers/l4/dispatcher.rs      choose Legacy or Flow path
  → layers/l4/context.rs         peek bytes, populate KvStore (conn.ip, conn.port, etc.)
  → layers/l4/flow.rs            enter Flow Engine at L4
  → engine/executor.rs           recursive middleware→branch→terminator execution
    → Plugin: protocol_detect    peek → set conn.detected_protocol
    → Plugin: upgrade("tls")     return TerminatorResult::Upgrade
  → layers/l4p/tls.rs            parse ClientHello (SNI, ALPN), inject tls.* into KV
  → layers/l4p/flow.rs           enter Flow Engine at L4+
    → Plugin: match(tls.sni)     branch by SNI
    → Plugin: upgrade("httpx")   return TerminatorResult::Upgrade
  → plugins/protocol/upgrader/decryptor.rs   TLS terminate via rustls
  → layers/l7/http/httpx.rs      hyper HTTP server, per-request Container
  → layers/l7/flow.rs            enter Flow Engine at L7
    → Plugin: match(req.path)    branch by path
    → Plugin: fetch_upstream     reverse proxy to backend
```

## Module Reference

### `src/bootstrap/` — Startup Orchestration
Sequence: crypto init → dotenv → logging → config load → watchers → certs → plugins → listeners → console → sigterm wait → shutdown.

| File | Role |
|---|---|
| `startup.rs` | 16-step boot sequence, `start()` is the real main |
| `logging.rs` | Log level from env |
| `console.rs` | Optional management API on unix socket or TCP |
| `monitor.rs` | Background memory monitoring for L7 adaptive behavior |
| `socket.rs` | Unix domain socket binding |

### `src/config/` — Configuration Management
Wraps `live` crate. Global singleton `CONFIG: OnceCell<ConfigManager>`.

| Struct | Backend | Config Path Pattern |
|---|---|---|
| `ListenerManager.tcp` | `LiveDir<TcpConfig>` | `listener/[port]/tcp.{toml,yaml,json}` |
| `ListenerManager.udp` | `LiveDir<UdpConfig>` | `listener/[port]/udp.{toml,yaml,json}` |
| `resolvers` | `LiveDir<ResolverConfig>` | `resolver/{name}.{toml,yaml,json}` |
| `applications` | `LiveDir<ApplicationConfig>` | `application/{name}.{toml,yaml,json}` |
| `nodes` | `Live<NodesConfig>` | `nodes.{toml,yaml,json}` |
| `lazycert` | `Live<LazyCertConfig>` | `lazycert.{toml,yaml,json}` |

All support hot-reload via `fsig` filesystem watcher → `atomhold` atomic swap. Invalid configs are rejected (keep-last-known-good).

### `src/ingress/` — Network Listeners
Owns socket lifecycle. `TASK_REGISTRY: DashMap<(u16, Protocol), RunningListener>` tracks active ports.

| File | Role |
|---|---|
| `listener.rs` | `start_listener` / `stop_listener` — bind with retry (5x) |
| `tcp.rs` | Accept loop → spawn `dispatch_tcp_connection` per conn |
| `udp.rs` | Recv loop → spawn handler per datagram |
| `hotswap.rs` | Subscribes to config change events, starts/stops listeners |
| `state.rs` | `Protocol` enum, `RunningListener`, `TASK_REGISTRY` |
| `tasks.rs` | Connection tracking, graceful shutdown token |

### `src/engine/` — Flow Execution Core
Generic recursive executor. One plugin per step, middleware branches to next step via `output` map.

| File | Role |
|---|---|
| `executor.rs` | `execute()` (with timeout) → `execute_recursive()` → dispatch to middleware/terminator trait |
| `context.rs` | `ExecutionContext` trait, `TransportContext` (L4/L4+), `ApplicationContext` (L7) |
| `interfaces.rs` | All plugin traits: `Plugin`, `Middleware`, `GenericMiddleware`, `HttpMiddleware`, `L7Middleware`, `Terminator`, `L7Terminator` |
| `key_scoping.rs` | `flow_path` based KV namespacing to isolate plugin outputs |

**Plugin dispatch priority:** `HttpMiddleware` → `GenericMiddleware` → `L7Middleware` → `Middleware` (legacy) → `L7Terminator` → `Terminator`.

**Circuit breaker:** External plugins get a passive quiet period (default 3s) after failure. Tracked in `EXTERNAL_PLUGIN_FAILURES: DashMap`.

### `src/layers/l4/` — Layer 4 (Transport)

| File | Role |
|---|---|
| `dispatcher.rs` | `dispatch_tcp_connection` — Legacy vs Flow path, handles `TerminatorResult::Upgrade` to L4+ |
| `context.rs` | Peek first bytes, populate KV (`conn.peek_hex`, byte analysis) |
| `flow.rs` | Sets `conn.layer=l4`, creates `TransportContext`, calls executor |
| `balancer.rs` | Target selection with health-check awareness |
| `resolver.rs` | DNS resolution for target hosts |
| `health.rs` | Target health registry |
| `session.rs` | UDP session tracking (client_addr → upstream mapping) |
| `proxy/tcp.rs` | Bidirectional TCP stream copy |
| `proxy/udp.rs` | UDP datagram forwarding |
| `proxy/stream.rs` | Generic `AsyncRead+AsyncWrite` proxy |
| `validator.rs` | Config validation |
| `legacy/` | Non-flow direct forwarding (preserved for backward compat) |

### `src/layers/l4p/` — Layer 4+ (Carrier: TLS/QUIC)

| File | Role |
|---|---|
| `tls.rs` | Smart peek loop for ClientHello, parse SNI/ALPN, run L4+ flow, handle upgrade to L7 |
| `plain.rs` | Plaintext passthrough (non-encrypted L4+ path) |
| `flow.rs` | Sets `conn.layer=l4p`, calls executor |
| `context.rs` | Inject `tls.*` / `quic.*` metadata into KV |
| `model.rs` | L4+ data models |
| `quic/` | QUIC protocol: `protocol.rs`, `session.rs`, `muxer.rs`, `virtual_socket.rs`, crypto/packet handling |

### `src/layers/l7/` — Layer 7 (Application: HTTP)

| File | Role |
|---|---|
| `flow.rs` | Thin wrapper → `executor::execute_l7` |
| `container.rs` | `Container` — full-duplex HTTP context (request + response + KV + protocol_data) |
| `model.rs` | Request/response models |
| `protocol_data.rs` | Protocol-specific abstractions |
| `http/httpx.rs` | Hyper HTTP/1.1+2 server, per-request Container creation |
| `http/h3.rs` | HTTP/3 handler |
| `http/wrapper.rs` | Protocol wrapper |

### `src/plugins/` — Plugin System

#### Core Infrastructure (`plugins/core/`)
| File | Role |
|---|---|
| `registry.rs` | `INTERNAL_PLUGIN_REGISTRY` (DashMap, compiled-in), `EXTERNAL_PLUGIN_REGISTRY` (atomhold Store, runtime-loaded) |
| `handler.rs` | External plugin execution (HTTP call, Unix socket, Command exec) |
| `loader.rs` | Plugin discovery and initialization |
| `external.rs` | External plugin config parsing |

#### Built-in Plugins

| Plugin Name | Type | Layer | File |
|---|---|---|---|
| `protocol_detect` | Middleware | L4 | `plugins/protocol/detect.rs` |
| `upgrade` | Terminator | L4/L4+ | `plugins/protocol/upgrader/upgrade.rs` |
| `common_match` | Middleware | Any | `plugins/middleware/matcher.rs` |
| `ratelimit_sec/min` | Middleware | Any | `plugins/middleware/ratelimit.rs` |
| `abort` | Terminator | L4 | `plugins/l4/abort.rs` |
| `proxy.transparent` | Terminator | L4 | `plugins/l4/proxy/ip.rs` |
| `proxy.domain` | Terminator | L4 | `plugins/l4/proxy/domain.rs` |
| `proxy.node` | Terminator | L4 | `plugins/l4/proxy/node.rs` |
| `fetch_upstream` | Terminator | L7 | `plugins/l7/upstream/mod.rs` |
| `send_response` | Terminator | L7 | `plugins/l7/response/mod.rs` |
| `static` | Terminator | L7 | `plugins/l7/static_files/mod.rs` |
| `cgi` | Terminator | L7 | `plugins/l7/cgi/mod.rs` |

#### Protocol Handling (`plugins/protocol/`)
- `tls/clienthello.rs` — TLS ClientHello parser (SNI, ALPN, cipher suites)
- `quic/packet.rs`, `frame.rs`, `crypto.rs` — QUIC Initial packet parsing
- `upgrader/decryptor.rs` — TLS termination via rustls, hands off to L7
- `upgrader/upgrade.rs` — `TerminatorResult::Upgrade` producer

### `src/resources/` — Shared Resources

| File/Dir | Role |
|---|---|
| `kv.rs` | `KvStore = AHashMap<String,String>` — per-connection context. Pre-populated with `conn.uuid`, `conn.ip`, `conn.port`, `conn.proto`, `conn.timestamp`, `server.ip`, `server.port` |
| `certs/loader.rs` | Scan `certs/` dir, load PEM/CRT/KEY files |
| `certs/format.rs` | Certificate format detection and conversion |
| `certs/arcswap.rs` | Atomic certificate swapping for hot-reload |
| `templates/parser.rs` | `{{variable}}` syntax parser (supports nesting: `{{kv.{{proto}}_backend}}`) |
| `templates/resolver.rs` | AST → string resolution with depth/size limits |
| `templates/context.rs` | `TemplateContext` trait, `SimpleContext` (KV-only), `L7Context` (hijack-capable) |
| `templates/hijack/` | Layer-specific variable providers (`l4p.rs` for TLS raw bytes, `l7_http.rs` for `req.body`, `req.header.*`, etc.) |
| `service_discovery/` | Service discovery model |

### `src/api/` — Management Console (feature-gated: `console`)
REST API via Axum. Access token auth. Optional Swagger UI.

Endpoints: ports, applications, flows, certs, nodes, resolvers, plugins, system info.

### `src/common/` — Shared Utilities

| File | Role |
|---|---|
| `config/env_loader.rs` | `get_env(key, default)` — env var with fallback |
| `config/file_loader.rs` | Config directory resolution (`VANE_CONFIG_DIR` or default) |
| `net/ip.rs` | Private IP range detection |
| `net/port_utils.rs` | Port validation |
| `sys/hotswap.rs` | Generic config watch loop (used by ingress) |
| `sys/lifecycle.rs` | Ensure config dirs exist, background tasks |
| `sys/system.rs` | System metrics (memory) |

### `src/lazycert/` — Optional ACME-like Certificate Integration

| File | Role |
|---|---|
| `config.rs` | LazyCert configuration |
| `client.rs` | External API client |
| `registry.rs` | Certificate registry |
| `sync.rs` | Periodic cert sync |

## External Crate Tree (reference/)

```
live (v0.4.3) — Live-reloading config controller
├── atomhold (v0.2.2) — Arc-based atomic state swap with versioning
├── fmtstruct (v0.2.6) — Format-agnostic config loader (TOML/YAML/JSON)
└── fsig (v0.2.4) — Filesystem signal (notify + debounce + glob filter)
```

Also uses `sigterm` crate for shutdown signal handling (separate from reference/).

## Key Design Patterns

1. **Flow Engine**: Config-driven DAG. Each node = one plugin. Middleware returns branch name → selects next node from `output` map. Terminator ends the flow or returns `Upgrade` to escalate layer.

2. **Layer Escalation**: L4 → L4+ → L7 via `TerminatorResult::Upgrade { protocol, conn }`. Connection object is passed up. KvStore accumulates metadata across layers.

3. **KvStore as Universal Context**: `AHashMap<String,String>` threaded through entire connection lifecycle. Template engine resolves `{{conn.ip}}`, `{{tls.sni}}`, `{{req.header.host}}` etc. from KV.

4. **Template Engine**: `{{var}}` with nesting support. Depth-limited (default 5), size-limited (default 64KB). L7 "hijack" resolvers intercept special prefixes (`req.body`, `req.header.*`) and lazily read from Container.

5. **Hot-Reload**: `live` crate watches filesystem → `atomhold` atomically swaps config → ingress `hotswap` loop diffs ports and starts/stops listeners.

6. **Plugin Dispatch**: Registry-based. Internal plugins compiled in. External plugins loaded at runtime from config, executed via HTTP/Unix socket/subprocess. Circuit breaker with passive quiet period.

7. **Dual Config Paths**: Legacy (direct forward) and Flow (engine-driven) coexist per listener. `TcpConfig` is an enum: `Legacy(LegacyTcpConfig)` | `Flow(FlowTcpConfig)`.

## Feature Flags (Cargo)

| Flag | What it gates |
|---|---|
| `tcp` / `udp` | Transport protocols |
| `tls` | TLS termination/passthrough (rustls) |
| `quic` | QUIC protocol support (quinn) |
| `httpx` | HTTP/1.1+2 (hyper) |
| `h2upstream` / `h3upstream` | Upstream proxy variants |
| `cgi` | CGI plugin |
| `static` | Static file serving plugin |
| `ratelimit` | Rate limiting middleware |
| `domain-target` | Domain-based routing |
| `console` | Management REST API |
| `swagger-ui` | OpenAPI docs UI |
| `lazycert` | External cert management |
| `aws-lc-rs` / `ring` | Crypto backend selection |
