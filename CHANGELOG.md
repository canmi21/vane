# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

## 0.0.2 (8. Nov, 2025)

- **added:** Integrated `tokio` (async runtime with full + signal features) to handle concurrent tasks and graceful shutdown.
- **added:** Introduced `axum` (v0.8 with macros) as the HTTP server framework for building management APIs.
- **added:** Enabled structured serialization/deserialization using `serde` and `serde_json`.
- **added:** Implemented environment variable loading via `dotenvy` for configuration management.
- **added:** Added `shellexpand` to support automatic expansion of `~/` and environment paths.
- **added:** Integrated `lazy-motd` for dynamic startup banner display.
- **added:** Added timestamp handling with `chrono` (with serde support) for structured logging and API responses.
- **added:** Unified and colorized startup log output using `fancy-log`.

## 0.0.1 (7. Nov, 2025)

- Initial release.
