<p align="center">
  <img src="https://raw.githubusercontent.com/canmi21/vane/refs/heads/latest/assets/vane.svg" alt="vane logo" width="300">
</p>

<p align="center">
  <b>Flow-based. Event-driven. Rust-native.<br/>Like a dandelion carried by the wind, it follows direction yet defines its own.</b>
</p>

<p align="center">
  <a href="https://crates.io/crates/vane"><img align="center" src="https://img.shields.io/crates/v/vane?style=flat&color=F09E64&labelColor=2D333B&logo=rust" alt="Crates.io version"/></a>
  <a href="https://github.com/canmi21/vane/blob/main/LICENSE"><img align="center" src="https://img.shields.io/github/license/canmi21/vane?style=flat&color=FF6B6B&labelColor=2D333B&logo=github" alt="License"/></a>
	<a href="https://deepwiki.com/canmi21/vane"><img align="center" src="https://deepwiki.com/badge.svg" alt="Ask DeepWiki"/></a>
  <a href="https://github.com/canmi21/vane/graphs/contributors"><img align="center" src="https://img.shields.io/github/contributors/canmi21/vane?style=flat&color=6FCF97&labelColor=2D333B&logo=github" alt="Contributors"/></a>
  <a href="https://github.com/canmi21/vane/actions"><img align="center" src="https://img.shields.io/github/actions/workflow/status/canmi21/vane/ci.yml?style=flat&color=3399FF&labelColor=2D333B&logo=githubactions" alt="Build Status"/></a>
  <a href="https://crates.io/crates/vane"><img align="center" src="https://img.shields.io/crates/d/vane?style=flat&color=9B5DE5&labelColor=2D333B&logo=rust" alt="Downloads"/></a>
  <a href="https://github.com/canmi21/vane/stargazers"><img align="center" src="https://img.shields.io/github/stars/canmi21/vane?style=flat&color=FFD43B&labelColor=2D333B&logo=github" alt="GitHub stars"/></a>
</p>

## What is Vane

Vane is a high-performance, flow-based reverse proxy and network protocol engine written in Rust. It is designed to bridge the architectural gap between raw transport layer (L4) forwarding and complex application layer (L7) processing. Unlike traditional reverse proxies that rely on static hierarchical configurations, Vane utilizes a dynamic, composable pipeline architecture that treats network connections as programmable flows.

## Core Concepts

### Flow-Based Pipeline

Vane abandons the traditional "virtual host" configuration model in favor of a decision-tree architecture known as the Flow Engine. Every connection operates within a pipeline composed of two distinct plugin types:

- **Middleware:** Intermediate logic units that inspect traffic, modify state, or perform side effects (e.g., protocol detection, rate limiting, variable injection). Middleware can branch execution paths based on runtime logic.
- **Terminators:** Final execution units that decide the fate of a connection (e.g., proxy to an upstream target, abort connection, or upgrade to a higher protocol layer).

### The Layered Stack Architecture

Vane manages network traffic across three strictly defined architectural layers, allowing for precise control over the depth of packet inspection:

- **L4 (Transport):** Handles raw TCP streams and UDP datagrams. It provides high-performance switching based on IP stickiness, load balancing, and connection metadata.
- **L4+ (Carrier):** A specialized state where Vane inspects encrypted or complex protocols (TLS, QUIC) without terminating the secure session. It can extract SNI, ALPN, and Connection IDs to make routing decisions before determining whether to forward the encrypted stream or terminate it.
- **L7 (Application):** The fully terminated layer where Vane acts as a server (HTTP/1.1, HTTP/2, HTTP/3). Here, the system utilizes a unified "Container" model to manipulate headers, bodies, and payloads using a full-duplex streaming engine.

### Two-Phase Dispatch (Fast/Slow Path)

To optimize performance for connection-oriented UDP protocols like QUIC, Vane implements a Two-Phase Dispatch system.

- **Slow Path:** Initial packets undergo deep packet inspection, flow evaluation, and cryptographic context assembly to determine the correct route.
- **Fast Path:** Once a session is established, Vane utilizes a global Connection ID (CID) Registry and IP Stickiness Map to perform O(1) forwarding for subsequent packets, bypassing the heavy flow engine entirely while maintaining NAT consistency.

## Distinctions

### Programmable vs. Configurable

Traditional proxies are configured; Vane is programmed. Through its plugin system, Vane allows administrators to define logic flows (e.g., "If protocol is HTTP and source IP is X, then rate limit, otherwise upgrade to HTTP/3"). This logic is defined in declarative JSON, YAML, or TOML, but executes with the speed of compiled Rust code.

### Hybrid Plugin Ecosystem

Vane offers a dual-layer extensibility model:

1. **Internal Plugins:** Compiled directly into the binary for critical, zero-latency operations (traffic shaping, protocol detection).
2. **External Plugins:** Supports execution of logic via HTTP webhooks, Unix Domain Sockets, or external binaries/scripts (Lua, Python, Bash, etc.). This allows integration with external authentication providers or logging systems without recompiling the core.

### Native QUIC & HTTP/3 Intelligence

Unlike proxies that treat UDP as a second-class citizen, Vane features a dedicated QUIC Carrier Engine. It includes custom virtual sockets, stream reassembly logic, and a specialized Muxer that allows the system to accept raw UDP packets, identify them as QUIC, and seamlessly transition them into a structured HTTP/3 application stream without losing context or performance.

## Technical Advantages

- **Zero-Copy Architecture:** The internal data plane heavily utilizes Rust's ownership model and `Bytes` abstractions to pass data between network layers without unnecessary memory allocation. Features like "Lazy Buffering" ensure that payloads are only loaded into memory when explicitly requested by a plugin.
- **Stateful L4+ Routing:** Vane can route TLS and QUIC traffic based on SNI and ALPN without possessing the SSL certificates. It parses the ClientHello during the handshake (even across fragmented QUIC packets) to make routing decisions, enabling true zero-trust routing.
- **Full-Duplex Streaming:** The upstream drivers are architected to handle large-scale data transfer (e.g., multi-gigabyte streams) asynchronously. Request and response paths are decoupled, preventing head-of-line blocking and deadlocks common in synchronous proxy implementations.
- **Hot-Swappable Configuration:** All layers of the stack—from L4 listeners and TLS certificates to L7 application pipelines—support runtime reconfiguration. The system employs a "Keep-Last-Known-Good" strategy to ensure stability during updates.

**✦** **`Polygon`** / **`Ethereum`**: `0x35D143d9DC624feC921a3925Fa84dea9d1DfDCAe`
If you found this project helpful, consider supporting domain & server maintenance.

---
> Canmi © 2025 [GitHub](https://github.com/canmi21) · [MIT](https://github.com/canmi21/vane?tab=MIT-1-ov-file#readme)
