# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

## 0.4.6 (13. Dec, 2025)

- **Changed:** Implemented **Flow Path Inheritance** for protocol upgrades. The L4 Flow Engine now calculates and passes the exact execution path (e.g., `...internal_transport_upgrade.tls`) to the L4+ engine via the `TerminatorResult`. This ensures KV namespace isolation remains consistent and collision-free across protocol layer transitions.
- **Changed:** Enhanced the `internal.common.match` middleware to support verbose operator aliases (`equal`, `not_equal`, `starts_with`, `ends_with`, `contain`) and added debug logging for runtime value comparisons.
- **Changed:** Upgraded error propagation across the Dispatcher, Proxy, and Flow engines. System logs now use the alternate formatting `{:#}` to display the full error chain (Context + Root Cause), exposing underlying I/O errors (e.g., "Connection reset by peer") previously masked by generic wrapper messages.
- **Fixed:** Replaced the manual `ClientHello` byte parser with the robust `tls-parser` crate. This resolves compilation type mismatches and ensures accurate extraction of complex TLS extensions (SNI, ALPN, Key Shares) regardless of field ordering or padding.

## 0.4.5 (13. Dec, 2025)

- **Added:** Introduced `internal.common.match`, a universal logic middleware. This plugin performs boolean comparisons between dynamic KVStore variables and static values, enabling flexible routing logic across all protocol layers. Supported operators now include verbose aliases:
  - Equality: `==`, `eq`, `equal`, `equals`
  - Inequality: `!=`, `ne`, `notequal`, `not_equal`
  - String ops: `contains`, `startswith`, `endswith`
- **Added:** Implemented comprehensive **TLS ClientHello Fingerprinting**. The system now parses and extracts the following metadata from the raw handshake:
  - **Basics:** SNI (Server Name Indication), ALPN (Application-Layer Protocol Negotiation), Legacy Protocol Version, Session ID, Random Bytes.
  - **Cryptographic Params:** Cipher Suites, Compression Methods, Supported Groups (Elliptic Curves), Signature Algorithms.
  - **Extensions:** Supported Versions (TLS 1.3), Key Share Groups, PSK Key Exchange Modes, Renegotiation Info.
  - **Security:** GREASE value detection (RFC 8701).
- **Changed:** Expanded the L4+ Context Injector (`context.rs`) to automatically populate the KVStore with the full spectrum of extracted TLS data (e.g., `tls.cipher_suites`, `tls.supported_groups`) immediately upon handshake detection.
- **Changed:** Removed the specialized `internal.protocol.tls.sni` and `internal.protocol.tls.alpn` plugins. These specific routing functions are now superseded by the combination of automatic Context Injection and the generic `internal.common.match` plugin.

## 0.4.4 (13. Dec, 2025)

- **Added:** Implemented advanced L4+ TLS routing capabilities with two new middleware plugins: `internal.protocol.tls.sni` and `internal.protocol.tls.alpn`. These plugins enable granular traffic filtering and branching based on Server Name Indication and Application-Layer Protocol Negotiation extensions without requiring TLS termination.
- **Added:** Enhanced the L4+ TLS Carrier to automatically peek and capture the raw `ClientHello` handshake message immediately after connection upgrade. The payload is hex-encoded and injected into the KVStore as `tls.clienthello`, serving as the foundational data source for upstream routing logic.
- **Added:** Introduced the `TLS_CLIENTHELLO_BUFFER_SIZE` environment variable (default: `4096` bytes). This allows operators to configure the initial socket peek window size, ensuring compatibility with clients sending unusually large handshake messages.
- **Changed:** Optimized the TLS Carrier entry point signature in `src/modules/stack/protocol/carrier/tls.rs`, removing unnecessary mutable bindings for the `TcpStream` handle during the context injection phase.

## 0.4.3 (13. Dec, 2025)

- **Breaking:** Refactored `TerminatorResult::Upgrade` to carry the active `ConnectionObject`. This architectural shift requires plugins to yield ownership of the underlying connection upon upgrade, ensuring the socket remains alive during L4-to-L4+ transitions.
- **Added:** Implemented L4+ Context Injection via the new `carrier/context.rs` module. This standardizes the population of protocol-specific metadata (e.g., `tls.sni`, `conn.layer`) into the KVStore immediately after a connection upgrade.
- **Changed:** Upgraded the Core Dispatcher (`dispatcher.rs`) to handle physical socket handover. It now captures the `ConnectionObject` returned by an Upgrade signal and spawns the specific Carrier logic (e.g., TLS runtime), completing the "Protocol Elevator" implementation.
- **Changed:** Updated the Flow Engine (`flow.rs`) and the `internal.transport.upgrade` plugin to support the new return signature, ensuring proper ownership transfer of `TcpStream` and `ByteStream` objects throughout the execution tree.

## 0.4.2 (11. Dec, 2025)

- **Breaking:** Refactored the internal plugin module structure. Moved `abort_connection.rs` to `abort.rs` and reorganized all transport proxy plugins into a modular `src/modules/plugins/terminator/transport/proxy/` directory to support complex multiprotocol implementations.
- **Added:** Introduced the `ByteStream` trait abstraction and the `ConnectionObject::Stream` variant. This enables the Flow Engine to pass generic, encrypted, or virtual streams (L4+) through the same pipeline as raw TCP/UDP sockets (L4).
- **Changed:** Upgraded the `internal.transport.proxy` plugin family (IP, Node, Domain) to support **Polymorphic Dispatching**. These plugins now automatically detect the underlying connection type (`TcpStream` vs `ByteStream`) and select the appropriate forwarding strategy, enabling seamless proxying for both raw L4 and upgraded L4+ protocols.
- **Changed:** Enhanced the Resolver configuration loader (`hotswap.rs`) with a **Keep-Last-Known-Good** fallback strategy. If a runtime configuration update contains conflicts or validation errors, the system now preserves the previous valid state instead of disabling the protocol, ensuring higher availability during config rollouts.

## 0.4.1 (11. Dec, 2025)

- **Added:** Implemented the L4+ Resolver Infrastructure. The system now actively scans and manages `config/resolver/{protocol}.{yaml|yml|json|toml}` configurations, allowing high-level protocol flows (e.g., TLS, HTTP) to be defined independently of physical L4 listeners.
- **Added:** Enforced a "Zero Tolerance" configuration policy for Resolvers. The loader now strictly prohibits parallel configuration files for the same protocol (e.g., simultaneous `tls.yaml` and `tls.json`). If conflicts are detected, the specific protocol is explicitly disabled to prevent undefined runtime behavior.
- **Changed:** Generalized the core configuration loader (`src/modules/stack/transport/loader.rs`). Extracted file reading, parsing, and validation into a reusable public `load_file` API, decoupling the loading logic from port-specific implementations.
- **Changed:** Integrated L4+ Resolvers into the system lifecycle. The bootstrap sequence and file watchers (`requirements.rs`) now initialize resolver states and monitor the `resolver/` directory for real-time updates, synchronizing changes via `ArcSwap`.
- **Changed:** Restructured the internal plugin hierarchy by moving the `upgrade` terminator to a dedicated `src/modules/plugins/terminator/upgrader/` directory, architecturaly separating layer-transition logic from standard transport terminators.

## 0.4.0 (10. Dec, 2025)

- **Breaking:** Redesigned the core `Terminator` plugin trait architecture (Terminator 2.0). The `execute` method signature has changed to return a `TerminatorResult` enum (supporting `Finished` or `Upgrade`) instead of a simple `Result`, enabling plugins to dictate complex flow control actions beyond simple termination.
- **Added:** Introduced the `internal.transport.upgrade` plugin. This critical component acts as an architectural bridge, allowing L4 connections to preserve their underlying socket state while transitioning control to higher-level L4+ (TLS) or L7 (HTTP) resolvers.
- **Added:** Implemented strict **Layer Awareness** in the plugin system. Terminators must now explicitly declare their supported architectural layers (`L4`, `L4Plus`, `L7`), and the configuration validator strictly enforces these constraints to prevent invalid cross-layer plugin usage.
- **Added:** Automatic injection of the `{{conn.layer}}` system variable (initializing as `"l4"` at the listener entry point), allowing flow logic and plugins to dynamically adapt behavior based on the current protocol depth.
- **Changed:** Upgraded the Flow Engine's recursive executor (`src/modules/stack/transport/flow.rs`) to support **Signal Bubbling**. The engine now captures and propagates termination signals from deep within the flow tree back to the central Dispatcher, enabling stateful protocol transitions.

## 0.3.7 (10. Dec, 2025)

- **Added:** Introduced two new core plugins for the Flow Engine: `internal.transport.proxy.node` and `internal.transport.proxy.domain`. These plugins enable direct L4 traffic termination to named Nodes (from `nodes.yaml`) or dynamic Domains (via DNS), streamlining complex routing configurations without requiring external scripts.
- **Changed:** Refactored the internal proxy architecture by extracting the core execution logic into a shared `proxy` module. This unifies the implementation of all transport terminators (IP, Node, Domain), reducing code duplication and ensuring consistent behavior across different target types.
- **Changed:** Deprecated the `internal.transport.proxy.transparent` plugin name in favor of the more concise `internal.transport.proxy`. The legacy name remains supported as an alias to maintain backward compatibility with existing flow configurations.
- **Changed:** Enhanced the `resolver` module to expose a public `resolve_domain_to_ips` helper function, allowing internal plugins to leverage the centralized, high-performance async DNS resolver for dynamic target resolution.

## 0.3.6 (9. Dec, 2025)

- **Changed:** Enhanced the `exec` plugin driver to capture `stderr` output from child processes. Plugin logs are now sanitized and piped directly into Vane's structured logging system at the `Debug` level, preventing console pollution while maintaining observability.
- **Fixed:** Resolved an input compatibility issue in the `exec` driver where line-buffered readers (e.g., Bash `read`, Java `Scanner`) failed to detect JSON payloads. The system now automatically appends a newline (`\n`) to the standard input stream, ensuring robust inter-process communication across all programming languages.

## 0.3.5 (8. Dec, 2025)

- **Added:** Implemented strict **KV Namespace Isolation** for the Flow Engine. Plugin outputs are now automatically namespaced using the unique execution path (`plugin.{flow_path}.{name}.{key}`), allowing identical plugins to be nested or reused multiple times within a complex flow tree without variable collision or state corruption.
- **Changed:** Refactored the recursive `Flow` executor (`src/modules/stack/transport/flow.rs`) to propagate a dynamic `flow_path` context string throughout the execution chain, establishing the architectural foundation for scoped state management.
- **Fixed:** Resolved a JSON deserialization mismatch in the External Plugin Registration API. The `ExternalPluginDriver` configuration now strictly enforces **Internal Tagging** (requiring a `"type"` discriminator field), fixing 400 Bad Request errors and aligning the server-side parser with the client-side JSON contract.

## 0.3.4 (4. Dec, 2025)

- **Changed:** Migrated the core DNS resolution dependency from the deprecated `trust-dns-resolver` to `hickory-resolver` (v0.25+). This modernization refactors the resolver initialization logic to utilize the new `TokioResolver::builder_with_config` pattern, aligning with the latest async IO standards.
- **Fixed:** Resolved the critical [**CVE-2024-12224**](https://rustsec.org/advisories/RUSTSEC-2024-0421) vulnerability in the dependency tree. By upgrading to `hickory-resolver`, Vane eliminates the risks associated with the flawed `idna` (v0.4.0) crate, ensuring robust protection against Punycode spoofing and homograph attacks in domain resolution.

## 0.3.3 (4. Dec, 2025)

- **Added:** Fully implemented the `http` and `unix` external plugin drivers. These drivers now function as fully compliant API clients, transmitting plugin inputs via POST requests and parsing responses according to a strict data contract.
- **Added:** Implemented a lightweight, dependency-free HTTP/1.1 client for the `unix` driver, enabling high-performance, low-latency communication with local plugins via Unix Domain Sockets without the overhead of a full HTTP stack.
- **Added:** Introduced the `EXTERNAL_HTTPS_CALL_SKIP_TLS_VERIFY` environment variable, allowing operators to globally disable TLS certificate verification for external HTTP plugins (e.g., when using self-signed certificates in internal networks).
- **Changed:** Enforced a standardized **API Response Contract** for all external plugins. To ensure reliability, external services must now return a unified JSON structure (`{ "status": "success", "data": ... }`), mirroring Vane's internal management API format.

## 0.3.2 (4. Dec, 2025)

- **Breaking:** Redesigned the external plugin execution model. The `bin` driver has been replaced by a more robust `command` driver. Configuration now requires explicit `program`, `args`, and `env` fields, enabling secure, shell-independent execution of binaries and interpreters (e.g., Python, Node.js) in restricted environments (Distroless/Musl).
- **Added:** Implemented the **JSON-over-Stdin** protocol for external command plugins. Vane now streams `ResolvedInputs` as a JSON payload to the child process's standard input and strictly parses standard output as `MiddlewareOutput` JSON, allowing for rich data exchange without command-line argument limits.
- **Added:** Integrated automatic **PATH resolution** for the `command` driver. Vane now intelligently searches the system `PATH` environment variable if the specified `program` is not a direct file path, simulating standard OS behavior.
- **Changed:** Architecturally decoupled the plugin execution logic into a dedicated `drivers` module (`exec`, `httpx`, `unix`), strictly enforcing the Single Responsibility Principle (SRP) and creating a foundation for future driver expansions.
- **Changed:** Updated the entire Plugin ecosystem to utilize `Cow<'static, str>` for parameter names and output branches, finalizing the transition to a zero-overhead, memory-safe string handling model.

## 0.3.1 (3. Dec, 2025)

- **Changed:** **Architectural Refactor:** Refactored the core `ParamDef` structure and `Middleware::output` signature to utilize `Cow<'static, str>` instead of `&'static str`. This creates a unified type system that efficiently handles both zero-cost static strings for built-in plugins and owned, garbage-collected strings for external plugins.
- **Fixed:** Eliminated a memory leak in the `ExternalPlugin` loader. Dynamic parameter names are no longer forced into `'static` lifetime via `Box::leak`, allowing for safe creation and destruction of external plugin definitions without residual memory usage.
- **Fixed:** Updated the recursive flow validator (`validator.rs`) to strictly handle Copy-on-Write string comparisons, ensuring accurate validation logic for mixed static/dynamic plugin environments.

## 0.3.0 (3. Dec, 2025)

- **Added:** Introduced the **External Plugin System**, enabling the integration of custom logic via three distinct drivers: `Http` (Remote Webhook), `Unix` (Local Socket), and `Bin` (Executable/Shell). This allows developers to extend Vane's functionality using any language or local tool.
- **Added:** Implemented a full RESTful management API (`/plugins`) supporting dynamic registration, updates, deletion, and listing of external plugins without requiring a service restart.
- **Added:** Integrated a persistent storage layer (`plugins.json`) which automatically saves and restores registered external plugins across system reboots.
- **Added:** Implemented strict connectivity validation for new plugins. HTTP endpoints are now verified via an `OPTIONS` request during registration to ensure availability. This check can be bypassed for development using the `SKIP_VALIDATE_CONNECTIVITY` environment variable.
- **Changed:** Enforced a critical security boundary: External plugins are now strictly limited to the `Middleware` role. The system explicitly rejects any attempt to register external code as a `Terminator` to protect core connection handling logic.
- **Fixed:** Resolved a potential startup failure where a newly created, empty `plugins.json` file would cause a JSON parsing error. The loader now detects zero-byte files and automatically initializes them with a valid default structure (`{}`).

## 0.2.6 (3. Dec, 2025)

- **Added:** Introduced two new high-performance rate-limiting middleware plugins: `internal.common.ratelimit.sec` and `internal.common.ratelimit.min`. These plugins enable precise traffic control based on arbitrary context keys (e.g., `{{conn.ip}}`), supporting separate counters for per-second and per-minute windows.
- **Added:** Implemented a robust memory management system for the rate limiters, configurable via `MAX_LIMITER_MEMORY` (default: 4MB).
- **Added:** Designed a "self-preservation" eviction strategy for the rate limiter pools. Instead of rejecting traffic when memory limits are reached, the system now randomly prunes approximately 10% of the oldest entries, preventing Out-Of-Memory (OOM) crashes while maximizing service availability under heavy load.
- **Added:** Integrated asynchronous background cleanup tasks that automatically reset rate-limit counters at 1-second and 60-second intervals, ensuring accurate window enforcement with zero impact on request latency.

## 0.2.5 (3. Dec, 2025)

- **Changed:** Enhanced the configuration diffing logic to perform a deep equality check (`PartialEq`) on loaded `TcpConfig` and `UdpConfig` objects.
- **Changed:** Implemented a new `RELOAD` lifecycle action. When a configuration change is detected for an active port, the listener is now automatically stopped and immediately restarted to apply the new settings, clearly logged as `↻ ... RELOAD (Config Changed)`.
- **Fixed:** Resolved a critical issue in the configuration hot-swap mechanism (`src/modules/ports/hotswap.rs`) where changes to the *content* of an existing listener's file (e.g., updating flow rules, targets, or logic) were ignored if the listener remained active. The system previously only tracked the presence of a configuration, failing to trigger updates for in-place modifications.

## 0.2.4 (2. Dec, 2025)

- **Added:** Significantly enhanced the `internal.protocol.detect` middleware with robust, multi-dimensional heuristic checks for **DNS** (validating QR bit, Opcode, and QDCOUNT) and **QUIC** (validating Fixed Bit and Version per RFC 9000), ensuring zero-collision protocol identification.
- **Changed:** Refined the UDP Flow Engine execution model to enforce strict **per-packet decision making**. Unlike the legacy sticky session mode, the Flow Engine now evaluates the plugin tree for every single datagram, using internal NAT mappings solely for routing return traffic from upstream backends.
- **Fixed:** Resolved a critical false-positive detection issue where DNS packets with specific random Transaction IDs (starting with `0xC0`) were misidentified as QUIC traffic due to overlapping header signatures.

## 0.2.3 (2. Dec, 2025)

- **Added:** Extended the Flow Engine to fully support UDP traffic. UDP listeners configured with the new `connection` format can now process datagrams through the plugin tree, enabling the same flexible middleware and terminator logic previously available only for TCP.
- **Added:** Implemented a dedicated `context` module (`src/modules/stack/transport/context.rs`) to centralize connection context initialization. This module strictly handles data peeking (TCP) or payload extraction (UDP) and populates the `KvStore` with standardized keys like `conn.proto` and `req.peek_buffer_hex`, adhering to the Single Responsibility Principle.
- **Changed:** Updated the `internal.transport.proxy.transparent` terminator plugin to support UDP connections. It now leverages a newly extracted `proxy_udp_direct` core function to handle UDP session management, NAT mapping, and bidirectional forwarding within the flow architecture.
- **Changed:** Refactored the UDP dispatch logic (`src/modules/stack/transport/proxy.rs`) to integrate with the Flow Engine. The dispatcher now dynamically branches execution between the legacy `protocols` list and the new `connection` tree based on the loaded configuration.

## 0.2.2 (26. Nov, 2025)

- **Added:** Implemented the core Flow Engine Executor (`src/modules/stack/transport/flow.rs`). This module recursively traverses the plugin tree, resolves `{{template}}` variables from the `KvStore`, executes `Middleware` and `Terminator` logic, and handles conditional branch routing.
- **Added:** Integrated the Flow Engine into the main TCP dispatcher. The system now intelligently peeks the initial connection payload, injects it into the `KvStore` (as `req.peek_buffer_hex`), and hands control to the new executor when a listener is configured with the `connection` format.
- **Fixed:** Resolved a critical runtime failure where the Flow Engine could not identify plugin types ("Plugin is neither a valid Middleware nor a Terminator"). This was due to Rust's inability to directly downcast `Box<dyn Plugin>` trait objects. The issue was fixed by adding safe `as_middleware` and `as_terminator` helper methods to the base `Plugin` trait.
- **Fixed:** Corrected the validation logic in `validator.rs` to utilize the new safe type coercion methods, ensuring that flow configurations are correctly validated against the plugin registry.

## 0.2.1 (26. Nov, 2025)

- **Changed:** Architecturally refactored the plugin registry into a dual-registry system. A static, immutable registry now safely houses all built-in plugins, while a new, atomically swappable (`ArcSwap`) registry has been introduced to support the future hot-reloading of external plugins.
- **Changed:** The global plugin lookup strategy (`get_plugin`) now prioritizes the internal registry, ensuring that core plugin functionality cannot be overridden by external plugins, thus enhancing system stability and security.
- **Fixed:** Corrected a series of critical compilation errors in the new flow-based configuration validator (`validator.rs`), primarily by replacing the direct `impl Validate` on an external type with a `#[validate(custom = "...")]` approach, thereby satisfying Rust's orphan rule.
- **Fixed:** Resolved a compilation failure in the plugin registry caused by a typo (`Lazy_new` instead of `Lazy::new`) during the initialization of static components.

## 0.2.0 (26. Nov, 2025)

- **Added:** Introduced a powerful, experimental flow-based processing engine as a new configuration format. Listeners can now be defined using a flexible, tree-like `connection` structure, enabling composable, multi-layer processing pipelines.
- **Added:** Architected a comprehensive plugin system, clearly distinguishing between `Middleware` (intermediate steps with named output branches) and `Terminator` (flow endpoints that finalize a connection).
- **Added:** Implemented a global, thread-safe Plugin Registry for dynamic lookup and validation of all built-in and future custom plugins.
- **Added:** Shipped the first set of internal plugins to power the new flow engine:
  - `internal.protocol.detect` (Middleware)
  - `internal.transport.abort` (Terminator)
  - `internal.transport.proxy.transparent` (Terminator)
- **Added:** Introduced a per-connection Key-Value store (`KvStore`) that attaches a unique context (UUID, source IP, etc.) to every connection, enabling stateful, context-aware processing across all plugin layers.
- **Changed:** **Architectural:** Refactored the core configuration models (`TcpConfig`, `UdpConfig`) into `enum`s. The system now seamlessly supports both the legacy `protocols` array and the new `connection` tree formats within the same listener file, ensuring full backward compatibility.
- **Changed:** The configuration validator (`validator.rs`) has been significantly enhanced to be dual-mode. It now includes a powerful, recursive validation engine for the new flow-based format, which cross-references the Plugin Registry to verify plugin names, required parameters, and data types at load time.
- **Changed:** Refactored the core L4 transparent proxy logic, extracting the TCP stream handling from `dispatcher.rs` into a reusable `proxy::proxy_tcp_stream` function. This function is now leveraged by both the legacy `forward` destination and the new `internal.transport.proxy.transparent` terminator plugin.
- **Changed:** The runtime dispatcher (`dispatcher.rs`) and UDP proxy (`proxy.rs`) are now aware of the dual-config format, branching their execution logic based on whether a listener is configured with `protocols` or a `connection` flow.
- **Fixed:** Resolved a fundamental architectural issue by introducing the `async-trait` crate. This makes the `Middleware` and `Terminator` plugin traits object-safe (`dyn Trait`), enabling their storage in a dynamic registry.
- **Fixed:** Corrected numerous compilation errors related to the `validator` crate, including fixing orphan rule violations by using a manual `impl Validate`, adding `#[validate(nested)]` where required, and using the correct error-handling APIs.
- **Fixed:** Addressed a wide range of compilation errors, including incorrect trait bounds (`PartialEq`, `Eq`), lifetime issues with static strings, and incorrect `lazy_static` pathing in derive macros, ensuring the codebase is fully compliant with the compiler.

## 0.1.16 (20. Nov, 2025)

- **Added:** Architecturally enhanced the connection handling pipeline by introducing a per-connection Key-Value store (`KvStore`). This foundational feature, managed by the new `modules/kv` module, automatically attaches essential metadata (`conn.uuid`, `conn.ip`, `conn.timestamp`, etc.) to every TCP and UDP connection upon creation, enabling advanced context-aware processing in future protocol layers.
- **Fixed:** Resolved a compilation error in the `kv` module by correcting the UUID generation method to `Uuid::now_v7`, which correctly uses the system's current time without requiring a manual timestamp argument.

## 0.1.15 (16. Nov, 2025)

- **Fixed:** The `nodes` configuration loader (`nodes/hotswap.rs`) now correctly recognizes both `.yml` and `.yaml` file extensions. This resolves a critical bug where a valid `nodes.yml` file was being ignored, causing the global node state to be empty and all `node:` type target resolutions to fail.
- **Changed:** Added more detailed debug logging to the node resolver (`resolver.rs`) to improve diagnostics when a node lookup fails.

## 0.1.14 (12. Nov, 2025)

- **Fixed:** Corrected a critical flaw in UDP mix-port forwarding where the session management mechanism was not protocol-aware. Previously, all datagrams from a single client IP:port were incorrectly locked to the backend of the first-matched protocol. The session key is now a composite of `(client_address, protocol_name)`, ensuring that different traffic types (e.g., DNS and QUIC) from the same client are correctly segregated and routed to their respective backends.
- **Changed:** The UDP session timeout is now configurable via the `UDP_SESSION_TIMEOUT_SECS` environment variable, defaulting to 30 seconds.

## 0.1.13 (11. Nov, 2025)

- **Fixed:** Corrected a critical deadlock in the UDP session handler (`proxy.rs`) that caused significant packet loss for traffic bursts from a single client. The session update logic could previously cause a lock contention, resulting in only the first packet of a sequence being proxied. The update mechanism is now fully atomic, ensuring reliable session affinity and correct handling of high-throughput UDP streams from a single source.

## 0.1.12 (11. Nov, 2025)

- **Fixed:** Corrected a flaw in the health-checking logic where a backend, once marked as "down" by a reactive connection failure, would not be automatically brought back into service by the periodic health checker. The state management in `health.rs` has been made more robust; the periodic checker now reliably overwrites the "unhealthy" status upon successful reconnection, ensuring that recovered backends are correctly returned to the active load-balancing pool.

## 0.1.11 (11. Nov, 2025)

- **Changed:** Enhanced the flexibility of the health-checking system (`health.rs`) by exposing its core timing parameters as environment variables. Operators can now fine-tune the TCP probe interval (`HEALTH_TCP_INTERVAL_SECS`), connection timeout (`HEALTH_TCP_CONNECT_TIMEOUT_MS`), and the UDP unhealthy target TTL (`HEALTH_UDP_UNHEALTHY_TTL_SECS`) without needing to recompile the application.
- **Fixed:** Resolved a significant TCP failover delay by implementing a reactive health-checking mechanism. Previously, the system would only detect a failed backend during its periodic health check cycle (every 5 seconds). Now, when a proxy connection attempt fails (e.g., `Connection refused`), the dispatcher immediately marks the target as unavailable. This ensures that subsequent traffic is instantly rerouted to healthy backends, dramatically improving reliability during runtime outages.

## 0.1.10 (10. Nov, 2025)

- **Fixed:** Resolved a critical bug in the file watcher (`requirements.rs`) where configuration changes in the `listener` directory were being ignored on macOS and other systems that use symbolic links for temporary directories (e.g., `/var` -> `/private/var`). The watcher now correctly canonicalizes paths before comparison, ensuring that hot-reloading for listeners is triggered reliably across all platforms.

## 0.1.9 (9. Nov, 2025)

- **Changed:** Architecturally refactored the L4 transport layer models (`src/modules/stack/transport/`) to enforce the Single Responsibility Principle. The original monolithic `model.rs` has been split into multiple, more focused files (`model.rs`, `tcp.rs`, `udp.rs`, `validator.rs`), significantly improving code organization and maintainability without introducing breaking changes to the configuration format.

## 0.1.8 (9. Nov, 2025)

- **Breaking:** Redesigned the forwarding target model (`Target`). It is now a flexible enum supporting `ip`, `domain`, or `node` types, requiring changes to all listener configuration files. For example, a target is now defined as `{ ip: "1.1.1.1", port: 80 }` or `{ domain: "example.com", port: 443 }`.
- **Added:** Integrated a new asynchronous DNS resolver module (`resolver.rs`) built upon `trust-dns-resolver`. This allows Vane to use domain names as backend targets and dynamically resolve them to IP addresses.
- **Changed:** The entire L4 proxy chain, including the health checker (`health.rs`) and load balancer (`balancer.rs`), has been refactored to be fully asynchronous. The balancer now resolves all `domain` and `node` targets into concrete IP addresses before applying health checks and balancing strategies.
- **Changed:** The health check mechanism is now resolver-aware. It periodically re-resolves all targets, enabling the system to automatically detect and adapt to changes in DNS records for `domain` targets and updates to the global `nodes` configuration.
- **Fixed:** Corrected a critical startup race condition. The bootstrap process now guarantees that the global `nodes` configuration is loaded before listener configurations, ensuring that `node`-type targets are always resolvable during initial health checks.

## 0.1.7 (9. Nov, 2025)

- **Added:** A new global `nodes` configuration system for service discovery. The application can now load a central list of named nodes from `nodes.yaml`, `nodes.json`, or `nodes.toml`.
- **Added:** Implemented a hot-swap mechanism for the `nodes` configuration. The application now watches the `nodes` file and reloads it automatically on change.
- **Changed:** The `nodes` data model has been redesigned to support a more flexible structure, allowing multiple IP configurations (with different ports and types) under a single named node.
- **Changed:** The application's file watcher has been re-architected to be context-aware. It now intelligently distinguishes between changes to `listener` configurations and the global `nodes` configuration, dispatching update signals to the correct modules.
- **Changed:** The IP address utility (`ip.rs`) has been refactore-d to use stable Rust methods for checking private IPv6 ranges, removing the dependency on [unstable](https://github.com/rust-lang/rust/issues/27709) nightly features.
- **Fixed:** Corrected a critical bug in the `nodes` loader where it would attempt to parse a file before checking for conflicts. The loader now correctly prioritizes the conflict check.
- **Fixed:** Resolved a compilation error by implementing the `Hash` trait for the `IpType` enum in the `nodes` data model.
- **Fixed:** Corrected a critical bug where the file watcher process would terminate prematurely, disabling all configuration hot-swap functionality.

## 0.1.6 (9. Nov, 2025)

- **Breaking:** The listener configuration directory structure has been changed. All port configurations (e.g., `[80]/`) must now reside within a `listener` subdirectory. The application will no longer scan the root config directory for listeners.
- **Changed:** The configuration hot-swap watcher is now context-aware. It only triggers a listener reload when changes are detected specifically within the `listener` subdirectory, improving efficiency.
- **Changed:** The application bootstrap process now ensures the `listener` configuration subdirectory exists on startup.

## 0.1.5 (9. Nov, 2025)

- **Breaking:** The `server` module has been fully restructured into a new `stack` architecture, separating protocol and transport layers for clearer layering and modularity.
- **Added:** Introduced L4–L7 layered directories (`l4`, `l5`, `l7`) to explicitly define network stack hierarchy.
- **Changed:** All protocol-related logic (`plain`, `quic`, `tls`) migrated under `stack/protocol`.
- **Changed:** Transport-related components (`balancer`, `proxy`, `session`) moved under `stack/transport`.
- **Changed:** Internal routing and module imports updated to reflect the new `stack` namespace.

## 0.1.4 (8. Nov, 2025)

- **Added:** Implemented full L4 UDP transparent proxy functionality, including a stateful session manager to maintain client-to-target affinity.
- **Added:** A new UDP session manager (`session.rs`) with a background cleanup task to prune idle sessions and enforce a configurable memory limit via `UDP_SESSION_BUFFER`.
- **Added:** A new IP utility (`ip.rs`) to apply different timeouts for private (`UDP_TIMEOUT_LOCAL`) versus public (`UDP_TIMEOUT_REMOTE`) upstream targets.
- **Changed:** The health checker now employs a reactive model for UDP, temporarily marking targets as unavailable on send failures.
- **Changed:** The load balancer has been updated to use the new reactive health status when selecting UDP targets.
- **Changed:** L4 proxy logic has been refactored into a dedicated `proxy.rs` module for better separation of concerns.
- **Fixed:** Corrected a critical bug where the UDP listener would not process any incoming datagrams due to an incorrect task structure.
- **Fixed:** The `prefix` detection method has been corrected to find a pattern anywhere within the initial data buffer, enabling proper detection of protocols like DNS.

## 0.1.3 (8. Nov, 2025)

- **Added:** A new `fallback` protocol detection method (`detect = { method = "fallback", pattern = "any" }`) to create unconditional catch-all rules, typically placed last in priority.
- **Added:** The configuration validator now enforces that the `pattern` for the `fallback` method must be `"any"`.
- **Changed:** The L4 dispatcher has been updated to correctly process the new `fallback` detection method.

## 0.1.2 (8. Nov, 2025)

- **Added:** A new `TCP_DETECT_LIMIT` environment variable (default: 64 bytes) to configure the size of the initial data buffer for L4 protocol detection, allowing for performance tuning.

## 0.1.1 (8. Nov, 2025)

- **Added:** Support for `regex` as a protocol detection method, allowing for more complex and precise matching of L4 traffic.
- **Changed:** The health check module has been refactored to distinguish between a blocking initial check and subsequent periodic checks.
- **Fixed:** Corrected a critical race condition in the application startup sequence. The initial health check now runs *after* the configuration has been loaded, eliminating the initial window where all targets were considered unavailable.

## 0.1.0 (8. Nov, 2025)

- **Added:** Implemented full L4 TCP transparent proxy functionality. After protocol detection, connections are now forwarded to healthy backend targets.
- **Added:** A background health checker that periodically monitors the availability and latency of all configured `targets` and `fallbacks` via non-intrusive TCP connection tests.
- **Added:** A load balancer with three configurable strategies (`Random`, `Serial`, `Fastest`) to select the optimal backend target based on real-time health data.
- **Added:** The load balancer now seamlessly fails over to `fallback` targets if all primary `targets` are determined to be unavailable by the health checker.
- **Changed:** The L4 dispatcher is now fully integrated with the load balancer, selecting a healthy target for each new connection based on its configured strategy.
- **Changed:** The application bootstrap process now initializes and starts the global health checker as a persistent background task.

## 0.0.13 (8. Nov, 2025)

- **Added:** An L4 connection dispatcher (`l4/dispatcher.rs`) responsible for protocol detection. The dispatcher implements `magic` (byte matching) and `prefix` (string matching) detection methods by peeking at the initial TCP stream data.
- **Changed:** Refactored the application's state management to use a globally accessible, static `CONFIG_STATE` to provide tasks with direct, thread-safe access to the current listener configuration.
- **Changed:** The TCP listener task in `ports/tasks.rs` now delegates incoming connections to the new L4 dispatcher for protocol analysis and routing.
- **Changed:** Updated the log format for matched protocols to `⇅ [{priority}] Matched protocol {protocol} for connection from {ip:port}` for improved clarity.

## 0.0.12 (8. Nov, 2025)

- **Added:** A new `server::l4::fs` module now centralizes all filesystem operations for listener configurations.
- **Added:** A new `server::l4::loader` module, residing within the L4 feature module, now handles the parsing and validation of configuration files.
- **Changed:** Refactored the L4 configuration architecture for a clearer separation of concerns. All configuration file loading, parsing, and filesystem logic has been moved from the `ports` module into the `server::l4` module.
- **Changed:** The `ports` module is now streamlined to be exclusively responsible for the runtime management and lifecycle of network listeners.

## 0.0.11 (8. Nov, 2025)

- **Breaking:** Removed support for the RON (`.ron`) configuration format due to persistent parsing issues with its underlying library. The supported formats are now TOML, YAML, and JSON.
- **Fixed:** Corrected a critical data model deserialization bug in `TcpDestination` and `UdpDestination` that prevented the parsing of nested `forward` blocks in all configuration formats.
- **Fixed:** Resolved a TOML parsing error by adjusting the expected configuration structure to correctly handle inline tables for `forward` type destinations.

## 0.0.10 (8. Nov, 2025)

- **Added:** A comprehensive L4 configuration system with distinct models for TCP and UDP listeners, defined in `src/modules/server/l4/model.rs`.
- **Added:** Support for loading listener configurations in four formats: TOML, YAML, JSON, and RON. The system automatically detects and parses the correct format.
- **Added:** Advanced, multi-level validation for all configuration files using the `validator` crate. This includes checks for unique priorities, required fields, regex patterns for names, and custom logical rules (e.g., `session` blocks are only valid for `resolver` destinations).
- **Added:** A generic configuration loader (`loader.rs`) responsible for parsing, pre-processing (e.g., lowercasing names), and validating configuration files.
- **Changed:** The configuration hot-swap mechanism is now powered by the new loader. It performs a full validation and parsing of changed files, only starting/stopping listeners if the new configuration is valid.
- **Changed:** The application will now detect and log warnings for conflicting configuration files (e.g., `tcp.toml` and `tcp.json` in the same directory) and deactivate the listener for that port and protocol to ensure safety.
- **Changed:** The internal `PortStatus` model has been upgraded to hold the complete, parsed `TcpConfig` or `UdpConfig` for each listener, providing the live configuration to the rest of the application.

## 0.0.9 (8. Nov, 2025)

- **Changed:** The application startup sequence has been refined. Dynamic port listeners are now initialized *after* the management console and network discovery have started, ensuring a cleaner and more logical boot order.
- **Changed:** Listener status logs (`UP`/`DOWN`) are now more descriptive, specifying whether the listener is binding to `IPv4` or dual-stack `IPv4 + IPv6`.
- **Fixed:** Corrected a critical bug where listeners configured at application startup were not being activated. The application now correctly scans and starts all required listeners when it first launches.

## 0.0.8 (8. Nov, 2025)

- **Added:** Implemented the core listener functionality. The application now actively binds to and listens on TCP and UDP ports as specified by the configuration files.
- **Added:** An asynchronous task manager to control the lifecycle of each network listener, spawning a dedicated Tokio task for each active protocol on a port.
- **Added:** A robust retry mechanism for port binding. If a port is already in use, the application will attempt to bind it again with an increasing backoff delay.
- **Added:** A global, thread-safe task registry (`DashMap`) to track the real-time state of all running listener tasks.
- **Added:** Listeners now support a graceful shutdown mechanism, entering a "draining" state to stop accepting new connections before terminating.
- **Changed:** The configuration hot-swap system is now fully connected to the task manager, starting and stopping live network listeners based on the detected file changes.
- **Changed:** Dynamic port listeners now respect the global `LISTEN_IPV6` environment variable, enabling binding to either IPv4-only or dual-stack IPv6 addresses.
- **Changed:** Refactored the `ports` module by splitting logic into `tasks.rs` (the workers), `listener.rs` (the manager), and `hotswap.rs` (the config link) for better separation of concerns.

## 0.0.7 (8. Nov, 2025)

- **Added:** A configuration hot-swap mechanism that automatically reloads listener settings when files in the config directory change.
- **Added:** The file watcher uses a 2-second debouncing period to ensure it only reloads after file operations have stabilized.
- **Added:** An in-memory, atomically swappable state (`ArcSwap`) now holds the live configuration of all ports, allowing for thread-safe reads and updates.
- **Added:** A new `GET /ports/{:port}` endpoint to retrieve the detailed live status (active state, protocols) of a single port from the in-memory state.
- **Added:** New endpoints `POST /ports/{:port}/{:protocol}` and `DELETE /ports/{:port}/{:protocol}` to manage individual TCP or UDP listeners for a port.
- **Changed:** The hot-swap logic now calculates a precise diff and logs declarative "UP" and "DOWN" messages for each listener that is added or removed.
- **Changed:** Application startup logic has been refactored into a `common/requirements.rs` module for better organization.
- **Fixed:** Corrected Axum router's state management by updating function signatures to `Router<PortState>` and using the `.with_state()` method properly during server initialization.

## 0.0.6 (8. Nov, 2025)

- **Added:** A new API endpoint group (`/ports`) for dynamically managing listener port configurations.
- **Added:** `GET /ports` to list all configured ports by scanning the configuration directory.
- **Added:** `POST /ports/{:port}` to create a new port listener by creating a corresponding `[<port>]` directory.
- **Added:** `DELETE /ports/{:port}` to remove a port listener by deleting its configuration directory.
- **Added:** A new request logging middleware that logs all incoming API calls, using `INFO` level for mutating requests (POST, DELETE) and `DEBUG` for read-only requests (GET).
- **Changed:** All API handlers now use the standardized JSON response format provided by the `response` module.
- **Changed:** Refactored API handler functions to follow a consistent naming convention (`{method}_{object}_handler`).

## 0.0.5 (8. Nov, 2025)

- **Added:** The management console now listens on a Unix domain socket (`/var/run/vane/console.sock` by default) in addition to the TCP port, configurable via the `SOCKET_DIR` environment variable.
- **Added:** Implemented automatic cleanup of the Unix socket file upon graceful shutdown.
- **Changed:** The server will now detect and remove a stale socket file from a previous run to prevent startup failures.
- **Fixed:** Refactored the graceful shutdown mechanism to use a central `tokio::sync::Notify`, resolving an issue where shutdown signals were processed multiple times, causing duplicate log messages.

## 0.0.4 (8. Nov, 2025)

- **Added:** A configuration management module (`getconf`) to handle config directory resolution and initialization.
- **Changed:** The application now automatically creates the configuration directory (`~/vane/` by default) and any necessary default config files on first run.

## 0.0.3 (8. Nov, 2025)

- **Added:** A utility module (`portool`) to validate network port numbers.
- **Added:** Configuration via `CONSOLE_LISTEN_IPV6` environment variable to enable api listening on IPv6 addresses.
- **Changed:** Refactored environment variable handling into a centralized `getenv` utility for improved reusability.
- **Changed:** Enhanced server startup logic to validate the listening port, with a fallback to a default value if the configured port is invalid.

## 0.0.2 (8. Nov, 2025)

- **Added:** Integrated `tokio` (async runtime with full + signal features) to handle concurrent tasks and graceful shutdown.
- **Added:** Introduced `axum` (v0.8 with macros) as the HTTP server framework for building management APIs.
- **Added:** Enabled structured serialization/deserialization using `serde` and `serde_json`.
- **Added:** Implemented environment variable loading via `dotenvy` for configuration management.
- **Added:** Added `shellexpand` to support automatic expansion of `~/` and environment paths.
- **Added:** Integrated `lazy-motd` for dynamic startup banner display.
- **Added:** Added timestamp handling with `chrono` (with serde support) for structured logging and API responses.
- **Added:** Unified and colorized startup log output using `fancy-log`.

## 0.0.1 (7. Nov, 2025)

- Initial release.
