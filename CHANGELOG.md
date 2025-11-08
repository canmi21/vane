# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

## 0.0.6 (8. Nov, 2025)

- **Added:** A new API endpoint group (`/ports`) for dynamically managing listener port configurations.
- **Added:** `GET /ports` to list all configured ports by scanning the configuration directory.
- **Added:** `POST /ports/:port` to create a new port listener by creating a corresponding `[<port>]` directory.
- **Added:** `DELETE /ports/:port` to remove a port listener by deleting its configuration directory.
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
