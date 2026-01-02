# Vane Developer Guide

**This document provides guidelines for contributing to Vane.**

For architectural details, see [ARCHITECTURE.md](ARCHITECTURE.md).
For deep reference documentation, see `docs/reference/`.

## 📚 Reference Documentation

-   [**01. Core Infrastructure**](docs/reference/01-core-infra.md): Bootstrap, Config Loading, Watchers.
-   [**02. L4 Transport**](docs/reference/02-l4-transport.md): TCP/UDP Listeners, Dispatcher, Proxy.
-   [**03. L4+ Carrier**](docs/reference/03-l4p-carrier.md): TLS, QUIC, Plaintext Inspection.
-   [**04. L7 Application**](docs/reference/04-l7-application.md): HTTPX, H3, Container, Flow Engine.
-   [**05. Plugin System**](docs/reference/05-plugin-system.md): Developing Middleware and Drivers.

---

## 🛠 Code Organization

### Mandatory Conventions

1.  **File Headers:** Every `.rs` file MUST start with:
    ```rust
    /* src/[path]/[filename].rs */
    ```
2.  **Imports:** Group external crates first, then internal `crate::` imports.
3.  **Module Structure:** Use `mod.rs` for module entry points.
    -   *Good:* `src/modules/plugins/l7/cgi/mod.rs`
    -   *Bad:* `src/modules/plugins/l7/cgi.rs`

### Naming Patterns

-   **Variables/Functions:** `snake_case`
-   **Types/Traits:** `PascalCase`
-   **Constants:** `SCREAMING_SNAKE_CASE`
-   **Files:** `snake_case.rs` (Avoid reserved keywords like `match`, `type`, `static` -> use `matcher.rs`, `model.rs`, `r#static.rs`).

---

## 🔄 Development Workflow

1.  **Make Changes:** Edit code.
2.  **Verify:** Run `cargo check` immediately.
3.  **Test:** Wait for approval before running full `cargo test`.
4.  **Format:** Ensure code is standard (rustfmt is usually enforced).

### Adding a New Plugin

1.  **Choose Location:**
    -   Logic? -> `src/modules/plugins/middleware/`
    -   Endpoint? -> `src/modules/plugins/terminators/`
    -   App Driver? -> `src/modules/plugins/l7/`
2.  **Implement Traits:** Implement `Plugin` + (`Middleware` OR `Terminator` OR `HttpMiddleware`).
3.  **Register:** Add to `src/modules/plugins/core/registry.rs`.

---

## 🔍 Debugging & Logging

Vane uses `fancy_log`.

-   `log(LogLevel::Debug, "...")`: High-frequency tracing.
-   `log(LogLevel::Info, "...")`: Operational milestones.
-   `log(LogLevel::Error, "...")`: Recoverable failures.

---

## 🧪 Testing Strategy

-   **Unit Tests:** Collocated in `mod tests` within the source file.
-   **Integration Tests:** Go-based framework in `integration/`.
