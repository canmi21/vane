# Vane LLM Agent Guide

This document defines coding standards, architectural conventions, and interaction guidelines for LLM agents working on the Vane codebase.

## Project Identity

Vane is a flow-based network protocol engine written in Rust. It bridges the gap between raw transport layer (L4) forwarding and complex application layer (L7) processing through a dynamic, composable pipeline architecture that treats network connections as programmable flows.

Vane operates as a protocol funnel: connections enter at L4 (TCP/UDP), optionally pass through L4+ (TLS/QUIC inspection), and can terminate at any layer. HTTP is one of many supported protocols; the architecture is designed for extensibility to DNS, gRPC, and other application protocols.

Core value proposition:
- Flow-based execution model using decision-tree architecture
- Programmable rather than merely configurable
- Zero-copy streaming architecture using Rust ownership model
- Multi-protocol support with protocol-agnostic routing at lower layers

## Architecture Overview

### Three-Layer Stack

Vane processes network traffic across three distinct layers:

- **L4 (Transport):** Handles raw TCP streams and UDP datagrams with minimal inspection
- **L4+ (Carrier):** Inspects encrypted protocols (TLS, QUIC) without termination to extract routing metadata (SNI, ALPN, Connection IDs)
- **L7 (Application):** Fully terminates protocols (HTTP/1.1, HTTP/2, HTTP/3) with complete request/response manipulation

### Flow-Based Execution

Every connection executes through a pipeline defined at runtime. The flow consists of:
- **Middleware:** Intermediate logic units that inspect, modify, or branch execution paths
- **Terminators:** Final execution units that determine connection fate (proxy, abort, upgrade)

### Plugin System

Plugins are categorized by extensibility:
- **Middleware:** Fully extensible (internal built-in or external via HTTP/Unix/Exec)
- **Terminators:** Built-in only (tightly coupled to data plane operations)

### Core Modules

- **stack:** Three-layer network stack implementation (L4/L4+/L7)
- **plugins:** Plugin registration, external plugin loader, trait definitions
- **kv:** Cross-layer key-value context store
- **nodes:** Upstream target configuration
- **ports:** Listener configuration and management
- **certs:** TLS certificate loading and hot-swapping

## Mandatory Code Style Rules

### File Headers

ALWAYS start every .rs file with this exact pattern:

```rust
/* src/[module-path]/[filename].rs */
```

This is 100% consistent across the codebase. No exceptions.

### Comment Styles

- Use `///` for public API documentation (functions, structs, traits, methods)
- Use `//` for implementation comments
- Use numbered comments for multi-step logic:

```rust
// 1. Parse the header
// 2. Extract the SNI
// 3. Route based on domain
```

### Naming Conventions

- **Modules and functions:** `snake_case` (e.g., `parse_client_hello`, `protocol_detect`)
- **Structs and enums:** `PascalCase` (e.g., `Container`, `MiddlewareOutput`, `TlsClientHelloData`)
- **Constants and statics:** `SCREAMING_SNAKE_CASE` (e.g., `CONFIG_STATE`, `TASK_REGISTRY`, `NODES_STATE`)

### Import Organization

Organize imports in this order:

```rust
use std::collections::HashMap;
use tokio::net::TcpStream;
use hyper::Request;

use crate::modules::plugins::model::Plugin;
use crate::modules::kv::KvStore;
```

1. External crates (std, tokio, hyper, serde, etc.)
2. Blank line
3. Internal crate imports using `crate::`

### Logging Conventions

Use `fancy_log` with emoji prefixes to categorize log messages:
- Checkmark for success
- X mark for errors
- Warning sign for warnings
- Gear for operations/processing
- Arrow for actions/execution
- Up arrow for startup/initialization
- Reload symbol for configuration reloading

Format: `log(LogLevel::Info, "Message here");`

## File Organization Pattern

Every Rust file follows this structure:

```rust
/* src/[path]/[file].rs */

use external_crate::Type;
use crate::internal::Module;

pub struct Thing {
    field: String,
}

impl Thing {
    pub fn new() -> Self {
        // implementation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests [what is being verified].
    #[test]
    fn test_thing() {
        // test code
    }
}
```

1. File path comment (first line, always)
2. Blank line
3. Use statements (external, blank line, internal)
4. Blank line
5. Type definitions and structs
6. Implementation blocks
7. Functions (public before private)
8. Test module at the end (if applicable)

## Development Constraints

### Language

All code, comments, and documentation must be in English. No mixed languages.

### No Time Estimates

Never add time estimates to code comments, documentation, or commit messages. Do not write things like "this will take 2 hours" or "should be quick".

### Hot-Reload Safety

All configuration-driven changes must support runtime reconfiguration. Use `arc-swap` patterns for atomic updates. The system implements "Keep-Last-Known-Good" strategy: if a configuration update fails, the previous working state is retained.

### Zero-Copy Preference

Prefer ownership transfer over cloning. Use `Bytes`, pass-by-move, and avoid unnecessary allocations. Example:

```rust
// Preferred
fn process(data: Bytes) -> Bytes {
    data
}

// Avoid
fn process(data: &[u8]) -> Vec<u8> {
    data.to_vec()
}
```

### Error Handling

Use `anyhow::Result` for error propagation and `.context()` for adding error context:

```rust
use anyhow::{Result, Context};

fn load_config(path: &str) -> Result<Config> {
    let contents = std::fs::read_to_string(path)
        .context("Failed to read configuration file")?;
    // ...
}
```

## Common Patterns to Follow

### Flow Execution

Flow execution uses recursive `ProcessingStep` traversal. A ProcessingStep is a map of plugin names to plugin instances. Each instance contains:
- Input parameters (resolved from templates)
- Output branches (mapping results to next steps)

Pattern:

```rust
pub type ProcessingStep = HashMap<String, PluginInstance>;

pub struct PluginInstance {
    pub input: HashMap<String, Value>,
    pub output: HashMap<String, ProcessingStep>,
}
```

### Plugin Traits

Five main plugin traits exist:
- `Plugin` - Base trait for all plugins
- `Middleware` - L4/L4+ middleware
- `L7Middleware` - L7-specific middleware
- `Terminator` - L4/L4+ terminator
- `L7Terminator` - L7-specific terminator

Implementation pattern:

```rust
#[async_trait]
pub trait Middleware: Plugin {
    async fn execute(&self, kv: &mut KvStore, conn: &ConnectionObject) -> MiddlewareOutput;
}
```

### Template Resolution

Templates use double-brace syntax: `{{key}}`. Resolution happens at runtime from the KV store.

Common variables:
- `{{conn.ip}}` - Connection source IP
- `{{conn.port}}` - Connection source port
- `{{req.path}}` - Request path (L7)
- `{{req.method}}` - Request method (L7)
- `{{req.header.host}}` - Request header value (L7)

### KV Store Access

The KV store is a simple `HashMap<String, String>` that propagates across all layers:

```rust
kv.insert("key".to_string(), "value".to_string());
let value = kv.get("key");
```

Standard keys:
- `conn.uuid` - Unique connection identifier
- `conn.ip` - Source IP address
- `conn.port` - Source port
- `conn.protocol` - Protocol type (tcp/udp)

### Async Patterns

Use `tokio::spawn` for concurrent tasks. Prefer structured concurrency:

```rust
let handle = tokio::spawn(async move {
    // task logic
});

let result = handle.await?;
```

## Layer Selection Guide

Choose the appropriate layer for new features:

- **L4 (Transport):** Use for raw TCP/UDP forwarding without protocol inspection. Examples: IP-based routing, load balancing, connection pooling.
- **L4+ (Carrier):** Use for encrypted protocol inspection without termination. Examples: SNI/ALPN-based routing, QUIC connection ID routing, TLS passthrough.
- **L7 (Application):** Use for full protocol termination. Examples: HTTP header manipulation, request body inspection, response generation, WebSocket upgrade.

## Rust Code Development Workflow

### After Modifying Rust Code

**MANDATORY WORKFLOW:**

1. **Run `cargo check`** immediately after any code modification
2. **Fix compilation errors** if any are reported
3. **Notify user** once `cargo check` passes (do NOT run tests automatically)
4. **Wait for user instruction** before proceeding with testing

**Critical Rules:**
- ❌ **NEVER run tests automatically** (user must explicitly request testing)
- ❌ **NEVER run `cargo test` without user approval**
- ❌ **NEVER run `cargo build` unless user requests it**
- ✅ **ALWAYS run `cargo check` after code changes**
- ✅ **ALWAYS wait for user to decide when to test**

**Example Flow:**
```
[LLM modifies code]
→ Run: cargo check
→ If errors: Fix them and run cargo check again
→ If no errors: "✓ Code compiles successfully. Ready for testing when you are."
→ Wait for user to say "run tests" or similar
```

---

## Testing Conventions

### Rust Unit Tests (Code-Block Level Only)

**Scope:** LLM may write unit tests ONLY for code-block level testing (individual functions, structs, small modules).

**When to Write Tests:**
- ✅ When user explicitly requests tests for specific code
- ✅ When adding new utility functions or data structures
- ✅ When fixing bugs (add regression test)

**When NOT to Write Tests:**
- ❌ Without user approval
- ❌ For integration-level behavior (use Go tests instead)
- ❌ For end-to-end flows (use Go tests instead)

**Available Test Dependencies:**
```toml
[dev-dependencies]
serial_test = "3"       # Sequential test execution
tempfile = "3"          # Temporary file/directory creation
temp-env = "0.3"        # Temporary environment variables
dirs = "6"              # Platform-specific directories
tower = "0.5"           # Service trait for testing
axum = "0.8"            # Web framework (for handler testing)
```

**Pattern:**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// Tests that the function correctly parses valid input.
    #[test]
    fn test_parse_valid_input() {
        // test logic
    }

    /// Tests that the function handles invalid input.
    #[tokio::test]
    async fn test_parse_invalid_input() {
        // async test logic
    }
}
```

Documentation format: `/// Tests [what is being verified].`

### Go Integration Tests

Go integration tests are the primary testing method. Located in `integration/tests/`, they:
1. Follow Rust structure definitions 100%
2. Use libraries to generate JSON/TOML/YAML configurations
3. Start the vane binary
4. Test end-to-end behavior

Tests are organized by layer:
- `l4/` - Transport layer tests
- `l4p/` - Carrier layer tests
- `l7/` - Application layer tests

### Python Tests (Deprecated)

Python tests in `tests/` are deprecated. Ignore them. They are kept for reference only.

## Documentation Standards

### Architecture Documentation

Architecture documentation resides in `docs/` and is intended for developers and LLM agents, not end users. Documentation should:
- Use objective, factual tone
- Explain technical rationale for design decisions
- Include enough detail for understanding but not be exhaustive
- Provide independent configuration examples

### TODO.md Management

`TODO.md` is managed 100% by Claude Code for tracking project tasks and future work. Guidelines:

- Keep only current tasks and future planned work
- Remove completed tasks when milestones are reached
- Update when user indicates milestone completion
- Use for tracking architectural improvements, code refactoring, and feature development
- Organize by priority and category

### Configuration Examples

When documenting configuration, provide independent examples. Do not reference `/examples` directory, as it contains legacy examples that are considered poor quality.

### CHANGELOG.md Management

When updating CHANGELOG.md for a new version release, follow these strict rules:

**Format Structure:**
- Use version format: `## X.Y.Z (DD. Mon, YYYY)` (e.g., `## 0.6.9 (30. Dec, 2025)`)
- Insert new version entry between `## Unreleased` and the previous version
- Each change must start with `- **Category:** Description`

**Category Order (MANDATORY):**
1. **Breaking** - Breaking changes that require user action
2. **Added** - New features and capabilities
3. **Changed** - Changes to existing functionality
4. **Fixed** - Bug fixes

**Rules:**
- Only use these four categories (Breaking, Added, Changed, Fixed)
- Categories must appear in the order listed above
- If a category has no changes, omit it entirely
- Each bullet point should be a single line describing one change
- Use objective, technical language describing what changed and why

**Example:**
```markdown
## 0.6.9 (30. Dec, 2025)

- **Added:** Implemented new feature X with capability Y.
- **Changed:** Refactored module Z to improve performance.
- **Fixed:** Resolved issue with component A causing error B.
```

## What NOT to Do

1. **Do not deviate from file header format.** Every .rs file must start with `/* src/[path]/[file].rs */`.
2. **Do not mix languages.** All code, comments, and documentation must be in English.
3. **Do not add time estimates.** Never write things like "this will take 2 hours" or "should be fast".
4. **Do not break hot-reload compatibility.** Use `arc-swap` patterns for configuration updates.
5. **Do not ignore existing code patterns.** When modifying code, follow the existing structure and conventions.
6. **Do not use emoji in documentation.** Documentation should be pure text without emoji.
7. **Do not use subjective language in documentation.** Avoid words like "powerful", "elegant", "simple", "best". State facts objectively.

## Additional Resources

- Source code: `/Users/canmi/Canmi/Project/vane/src/`
- Architecture docs: `/Users/canmi/Canmi/Project/vane/docs/`
- Integration tests: `/Users/canmi/Canmi/Project/vane/integration/tests/`
- License: MIT
