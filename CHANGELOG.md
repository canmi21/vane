# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

## 0.0.9 (8. Nov, 2025)

- **Fixed:** Corrected a critical bug where listeners for ports that were already configured at application startup were not being activated. The application now correctly scans and starts all required listeners from the existing configuration when it first launches.

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
