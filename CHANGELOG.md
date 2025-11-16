# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

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
