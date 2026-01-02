# Vane Architecture Guide

**Version:** 0.9.0 (Planned)
**Last Updated:** 2026-01-02

Vane is a high-performance, flow-based network protocol engine written in Rust. Unlike traditional reverse proxies (Nginx, HAProxy) that use static configuration hierarchies, Vane uses a **Dynamic Flow Engine** where routing decisions are composed at runtime via a tree of plugins.

---

## 1. High-Level Design

### The "Protocol Elevator"

Vane operates on the principle of **Lazy Inspection**. Connections start at the lowest possible layer (L4) and are "promoted" up the stack only when necessary.

```mermaid
graph TD
    L4[L4 Transport<br>TCP/UDP] -->|Peek & Detect| Dispatcher
    Dispatcher -->|Match| Proxy[L4 Proxy<br>(Raw Forwarding)]
    Dispatcher -->|Upgrade| L4P[L4+ Carrier<br>TLS / QUIC]
    L4P -->|Decrypt & Inspect| Carrier
    Carrier -->|Match| Passthrough[Encrypted Passthrough]
    Carrier -->|Upgrade| L7[L7 Application<br>HTTP / Container]
    L7 -->|Process| Middleware[L7 Middleware<br>Auth / WAF]
    Middleware -->|Route| Driver[L7 Driver<br>Static / CGI / Upstream]
```

### Key Concepts

-   **Flow Engine:** A recursive executor that traverses a tree of `ProcessingSteps`. Each step is a Plugin.
-   **KV Store:** A per-connection, cross-layer hash map (`HashMap<String, String>`) that holds metadata (`conn.ip`, `tls.sni`, `http.path`).
-   **Container:** The L7 Envelope. Holds the high-fidelity Request/Response objects (Headers, Body Streams) and Protocol Data.
-   **Plugins:** Atomic logic units.
    -   **Middleware:** Returns a branch name (e.g., "success", "failure", "true", "false").
    -   **Terminator:** Ends the flow or signals an Upgrade.

---

## 2. Directory Structure (Source of Truth)

The codebase follows a strict layered architecture:

```text
src/
├── main.rs                    # Entry point
├── core/                      # Infrastructure (Bootstrap, Router, Socket)
├── common/                    # Utilities (Config Loader, IP, Watchers)
├── modules/
│   ├── certs/                 # TLS Certificate Management
│   ├── flow/                  # Core Flow Engine (Layer Agnostic)
│   ├── kv/                    # KV Store Implementation
│   ├── nodes/                 # Service Discovery (Nodes Registry)
│   ├── plugins/               # The Business Logic
│   │   ├── core/              # Plugin Traits & Registry
│   │   ├── middleware/        # Logic Plugins (Match, RateLimit)
│   │   ├── terminators/       # L4/L4+ Endpoints (Proxy, Abort)
│   │   └── l7/                # Application Drivers (CGI, Static, Upstream)
│   ├── ports/                 # L4 Listeners (Bind Logic)
│   └── stack/                 # The Network Stack
│       ├── transport/         # L4: TCP/UDP Dispatch & Legacy Proxy
│       ├── carrier/           # L4+: TLS & QUIC Inspection
│       └── application/       # L7: HTTP Engines & Container
└── ...
```

---

## 3. Data Flow & Lifecycles

### 3.1 L4 Transport (TCP/UDP)
-   **Entry:** `modules/ports` spawns listeners.
-   **Dispatch:** `stack/transport/dispatcher.rs` accepts sockets.
-   **Detection:** Peeks initial bytes (TCP) or payload (UDP).
-   **Decision:** Executes L4 Flow.
    -   *Terminator:* `proxy` (NAT/Tunnel).
    -   *Upgrade:* Spawns `stack/carrier` task.

### 3.2 L4+ Carrier (TLS/QUIC)
-   **Entry:** `stack/carrier/tls.rs` or `quic.rs`.
-   **Context:** Parses Handshake (ClientHello/Initial). Injects into KV (`tls.sni`).
-   **Decision:** Executes L4+ Flow (`resolver/tls.yaml`).
    -   *Terminator:* `proxy` (SNI Routing).
    -   *Upgrade:* Handover to `stack/application`.

### 3.3 L7 Application (HTTP)
-   **Entry:** `stack/application/http/httpx.rs` (Hyper) or `h3.rs` (Quinn).
-   **Container:** Wraps the stream in a `Container`.
-   **Decision:** Executes L7 Flow (`application/https.yaml`).
-   **Driver:** `FetchUpstream` (Reverse Proxy) or `Static` (File Server).
-   **Response:** Signals headers/body back to the adapter.

---

## 4. Configuration Model

Vane uses a **Dual-Mode** configuration system:

1.  **Legacy (L4 Only):** Simple list of `protocols` and `rules`.
2.  **Flow (Universal):** Tree-based `connection` structure.

```yaml
# Flow Example
connection:
  internal.protocol.detect:
    input:
      timeout: 500
    output:
      http:
        internal.transport.upgrade:
          input:
            protocol: "http"
      tls:
        internal.transport.upgrade:
          input:
            protocol: "tls"
```

## 5. Security Model

-   **Memory Safety:** 100% Safe Rust in Data Plane (unwrap-free).
-   **Isolation:** Plugins cannot corrupt the stack memory (ownership rules).
-   **Resource Control:**
    -   Rate Limiters (Memory-bounded).
    -   Flow Timeouts.
    -   Configurable Buffer Limits (`L7_MAX_BUFFER_SIZE`).
-   **External Plugins:** Sandboxed execution via `command` driver (Trusted Bin Root).
