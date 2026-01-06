# Vane LLM Working Guidelines

This document defines universal guidelines for any LLM working on the Vane codebase. It explains what you can do, how to do it correctly, and what to avoid.

## Project Context

Vane is a flow-based network protocol engine written in Rust. It operates as a protocol funnel across three layers (L4 Transport, L4+ Carrier, L7 Application) with a dynamic, composable pipeline architecture.

Key concepts:
- **Flow-based execution**: Decision-tree architecture with runtime-defined pipelines
- **Plugin system**: Middleware (extensible) and Terminators (built-in only)
- **Zero-copy streaming**: Rust ownership model, no unnecessary allocations
- **Cross-layer context**: KV store propagates metadata from L4 to L7

**Source location:** `/Users/canmi/Canmi/Project/vane/src/`

---

## What You Can Do

### Code Development

✅ Write new Rust code following existing patterns
✅ Refactor existing code for better architecture
✅ Add new plugins (internal or external middleware drivers)
✅ Fix bugs and optimize performance
✅ Write unit tests when user explicitly requests them
✅ Modify configuration schemas (JSON/YAML/TOML)
✅ Update architecture documentation in `docs/`
✅ Manage TODO.md task tracking (100% LLM-managed)

### Analysis & Review

✅ Read and analyze any source file
✅ Explain code behavior and design decisions
✅ Identify security vulnerabilities or design issues
✅ Review code quality and suggest improvements
✅ Trace data flow across layers
✅ Debug issues using code inspection

---

## How to Work Correctly

### 1. Mandatory Code Style Rules

#### File Headers
Every `.rs` file MUST start with this exact pattern:

```rust
/* src/[module-path]/[filename].rs */
```

No exceptions. This is 100% consistent across the codebase.

#### Comment Styles
- `///` for public API documentation (functions, structs, traits)
- `//` for implementation comments
- Numbered comments for multi-step logic:

```rust
// 1. Parse the header
// 2. Extract the SNI
// 3. Route based on domain
```

#### Naming Conventions
- **Modules/functions**: `snake_case` (e.g., `parse_client_hello`)
- **Structs/enums**: `PascalCase` (e.g., `Container`, `MiddlewareOutput`)
- **Constants/statics**: `SCREAMING_SNAKE_CASE` (e.g., `CONFIG_STATE`)

#### Import Organization
```rust
// External crates first
use std::collections::HashMap;
use tokio::net::TcpStream;

// Blank line
// Internal crate imports
use crate::modules::plugins::model::Plugin;
use crate::resources::kv::KvStore;
```

#### Logging Conventions
Use `fancy_log` with emoji prefixes:
- `✓` (Checkmark): Success
- `✗` (X mark): Errors
- `⚠` (Warning sign): Warnings
- `⚙` (Gear): Operations/processing
- `➜` (Arrow): Actions/execution
- `↑` (Up arrow): Startup/initialization
- `↓` (Down arrow): Shutdown/unloading
- `↻` (Reload symbol): Configuration reloading

Format: `log(LogLevel::Info, "Message here");`

---

### 2. File Organization Pattern

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

**Order:**
1. File path comment (first line, always)
2. Blank line
3. External imports
4. Blank line
5. Internal imports (using `crate::`)
6. Blank line
7. Type definitions and structs
8. Implementation blocks
9. Functions (public before private)
10. Test module at the end (if applicable)

---

### 3. Development Constraints

#### Language
All code, comments, and documentation MUST be in English. No mixed languages.

#### No Time Estimates
NEVER add time estimates to code, documentation, or commit messages. Do not write things like "this will take 2 hours" or "should be quick".

#### Hot-Reload Safety
All configuration-driven changes MUST support runtime reconfiguration. Use `arc-swap` patterns for atomic updates. The system implements "Keep-Last-Known-Good" strategy.

#### Zero-Copy Preference
Prefer ownership transfer over cloning:

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

#### Error Handling
Use `anyhow::Result` and `.context()`:

```rust
use anyhow::{Result, Context};

fn load_config(path: &str) -> Result<Config> {
    let contents = std::fs::read_to_string(path)
        .context("Failed to read configuration file")?;
    // ...
}
```

---

### 4. Common Patterns

#### Flow Execution
```rust
pub type ProcessingStep = HashMap<String, PluginInstance>;

pub struct PluginInstance {
    pub input: HashMap<String, Value>,
    pub output: HashMap<String, ProcessingStep>,
}
```

Flow execution uses recursive `ProcessingStep` traversal.

#### Plugin Traits
Five main traits:
- `Plugin` - Base trait
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

#### Template Resolution
Templates use `{{key}}` syntax, resolved at runtime from KV store.

Common variables:
- `{{conn.ip}}` - Connection source IP
- `{{conn.port}}` - Connection source port
- `{{req.path}}` - Request path (L7)
- `{{req.method}}` - Request method (L7)
- `{{req.header.host}}` - Request header (L7)

#### KV Store Access
```rust
kv.insert("key".to_string(), "value".to_string());
let value = kv.get("key");
```

Standard keys:
- `conn.uuid` - Unique connection ID
- `conn.ip` - Source IP
- `conn.port` - Source port
- `conn.protocol` - Protocol type (tcp/udp)

#### Async Patterns
```rust
let handle = tokio::spawn(async move {
    // task logic
});

let result = handle.await?;
```

---

### 5. Layer Selection Guide

Choose the appropriate layer:

- **L4 (Transport)**: Raw TCP/UDP forwarding without protocol inspection
  - Examples: IP-based routing, load balancing, connection pooling

- **L4+ (Carrier)**: Encrypted protocol inspection without termination
  - Examples: SNI/ALPN routing, QUIC connection ID routing, TLS passthrough

- **L7 (Application)**: Full protocol termination
  - Examples: HTTP header manipulation, body inspection, response generation, WebSocket upgrade

---

### 6. Rust Code Development Workflow

#### After Modifying Rust Code

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

### 7. Testing Conventions

#### Rust Unit Tests (Code-Block Level Only)

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

    /// Tests async behavior.
    #[tokio::test]
    async fn test_async_function() {
        // async test logic
    }
}
```

Documentation format: `/// Tests [what is being verified].`

#### Go Integration Tests (Primary)
Located in `integration/tests/`, organized by layer:
- `l4/` - Transport layer tests
- `l4p/` - Carrier layer tests
- `l7/` - Application layer tests

Integration tests:
1. Follow Rust structure definitions 100%
2. Generate configurations (JSON/TOML/YAML)
3. Start vane binary
4. Test end-to-end behavior

#### Python Tests (Deprecated)
Ignore Python tests in `tests/` directory. They are kept for reference only.

---

### 7. Documentation Standards

#### Architecture Documentation
Located in `docs/`, intended for developers and LLM agents (not end users).

Documentation MUST:
- Use objective, factual tone
- Explain technical rationale for design decisions
- Include sufficient detail but not be exhaustive
- Provide independent configuration examples
- Be in English only
- Avoid emoji
- Avoid subjective language (no "powerful", "elegant", "simple", "best")

#### TODO.md Management
`TODO.md` is managed 100% by LLM agents. Guidelines:
- Keep only current tasks and future planned work
- Remove completed tasks when milestones are reached
- Update when user indicates completion
- Organize by priority and category
- Task details stored in `.todo/` directory

#### Configuration Examples
When documenting configuration:
- Provide independent examples
- DO NOT reference `/examples` directory (legacy, poor quality)
- Show complete working examples

#### CHANGELOG.md Management
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

---

## What NOT to Do

### Critical Prohibitions

❌ **Do NOT deviate from file header format**
- Every `.rs` file MUST start with `/* src/[path]/[file].rs */`

❌ **Do NOT mix languages**
- All code, comments, documentation must be in English

❌ **Do NOT add time estimates**
- Never write "this will take 2 hours" or "should be fast"

❌ **Do NOT break hot-reload compatibility**
- Use `arc-swap` patterns for configuration updates

❌ **Do NOT ignore existing code patterns**
- Follow existing structure and conventions when modifying code

❌ **Do NOT use emoji in documentation**
- Documentation should be pure text (emoji only in logs)

❌ **Do NOT use subjective language in documentation**
- Avoid "powerful", "elegant", "simple", "best"
- State facts objectively

❌ **Do NOT reference /examples directory**
- It contains legacy examples considered poor quality
- Write independent configuration examples instead

❌ **Do NOT modify TODO.md manually**
- It is 100% managed by LLM agents
- Project owner does not edit it directly

❌ **Do NOT run tests automatically**
- Never run `cargo test` without user approval
- Never run `cargo build` unless user requests it
- Always run `cargo check` after code changes
- Wait for user instruction before testing

---

## Workflow Guidelines

### Before Making Changes

1. **Read existing code** - Understand current implementation
2. **Check documentation** - Review `docs/` for architecture context
3. **Review TODO.md** - Check if task is already planned
4. **Ask for confirmation** - Clarify requirements before implementing

### While Making Changes

1. **Follow code style** - File headers, naming, imports, comments
2. **Run `cargo check`** - After each code modification (see Section 6: Rust Code Development Workflow)
3. **Update documentation** - Keep `docs/` in sync with code changes
4. **Write tests** - Only when user explicitly requests them

### After Making Changes

1. **Verify compilation** - Run `cargo check` (NEVER run tests automatically)
2. **Notify user** - Inform when code compiles successfully
3. **Wait for user** - Let user decide when to run tests
4. **Update TODO.md** - Mark tasks as complete when user confirms
5. **Document changes** - Update relevant documentation files

---

## Additional Resources

- **Source code**: `src/`
- **Architecture docs**: `docs/`
- **Integration tests**: `integration/tests/`
- **TODO tracking**: `TODO.md`
- **License**: MIT

---

## Questions?

If unclear about any guideline:
1. Check existing code for patterns
2. Review architecture documentation in `docs/`
3. Ask the user for clarification
4. Follow the principle: "When in doubt, follow existing conventions"