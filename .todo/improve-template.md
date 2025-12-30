# Task 1.5: Template System Upgrade - Unified & Enhanced

**Status:** Design Complete, Awaiting Approval

**User Input:** 模板系统需要支持字符串拼接和嵌套解析，且需要统一三层的实现

**Blocker:** None (can implement independently)

---

## Current State Analysis

### Existing Implementations

**L4 Layer** (`src/modules/stack/transport/flow.rs`):
- Simple inline `resolve_inputs` function (~20 lines)
- Only supports `{{key}}` single-level replacement
- Only looks up from KV Store
- Does not support: nesting, concatenation, hijacking

**L4+ Layer** (`src/modules/stack/protocol/carrier/flow.rs`):
- **Exact duplicate code** (~20 lines)
- Only supports `{{key}}` single-level replacement
- Only looks up from KV Store

**L7 Layer** (`src/modules/stack/protocol/application/template.rs`):
- Dedicated module (120 lines)
- Supports recursive JSON structure resolution
- Supports "magic words": `req.body`, `req.header.*`, `res.*`
- Supports "hijacking": accessing `req.body` triggers lazy buffering
- **Still does not support string concatenation or nested templates**

### Identified Problems

1. ❌ **Code duplication** between L4 and L4+ (~20 lines × 2)
2. ❌ **No string concatenation** support across all layers (e.g., `"{{conn.ip}}:{{conn.port}}"`)
3. ❌ **No nested template** support across all layers (e.g., `"{{kv.{{conn.protocol}}_backend}}"`)
4. ❌ **L7 hijacking mechanism** hardcoded in `resolve_key` function (difficult to extend)

---

## Design Goals

1. ✅ **Unified parsing engine**: Eliminate L4/L4+/L7 code duplication
2. ✅ **String concatenation support**: `"{{a}}:{{b}}"` → `"value_a:value_b"`
3. ✅ **Nested template support**: `"{{kv.{{proto}}_backend}}"` → resolve inner first, then outer
4. ✅ **Layered context abstraction**: Use trait to support different layers' data access patterns
5. ✅ **Preserve hijacking mechanism**: L7 layer accessing `req.body` still triggers buffering

---

## Unified Architecture Design

### Module Structure

```
src/modules/template/
├── mod.rs              # Public API and utilities
├── parser.rs           # Lexer + Parser (AST generation)
├── resolver.rs         # Resolver (AST evaluation)
├── context.rs          # TemplateContext trait and implementations
└── hijack/             # Hijacking logic (layer + protocol specific)
    ├── mod.rs          # Hijacker trait and registry
    ├── l7_http.rs      # L7+HTTP hijacking (req.body, req.header.*, etc.)
    └── l7_dns.rs       # L7+DNS hijacking (future: dns.query.name, etc.)
```

### Core Components

#### 1. AST Definition (parser.rs)

```rust
/* src/modules/template/parser.rs */

/// Template AST node
pub enum TemplateNode {
    /// Plain text segment
    Text(String),

    /// Variable reference {{...}}
    Variable {
        /// Can contain nested nodes for concatenation/nesting
        parts: Vec<TemplateNode>,
    },
}

/// Parse template string into AST
pub fn parse_template(input: &str) -> Result<Vec<TemplateNode>> {
    // Lexer: Tokenize "{{", "}}", and text
    // Parser: Build nested structure
    // Supports:
    //   - "plain text" → [Text("plain text")]
    //   - "{{key}}" → [Variable{parts:[Text("key")]}]
    //   - "{{a}}:{{b}}" → [Variable{parts:[Text("a")]}, Text(":"), Variable{parts:[Text("b")]}]
    //   - "{{kv.{{proto}}_backend}}" → [Variable{parts:[Text("kv."), Variable{...}, Text("_backend")]}]
}
```

**Parsing Rules:**
- Parse from innermost to outermost
- Support arbitrary nesting depth
- Text mixing: inner results concatenate with plain text before outer resolution
- Fail-fast: if any resolution fails, entire template fails

**Examples:**

| Input | AST |
|-------|-----|
| `"plain text"` | `[Text("plain text")]` |
| `"{{key}}"` | `[Variable{parts:[Text("key")]}]` |
| `"{{a}}:{{b}}"` | `[Variable{parts:[Text("a")]}, Text(":"), Variable{parts:[Text("b")]}]` |
| `"{{kv.{{proto}}_backend}}"` | `[Variable{parts:[Text("kv."), Variable{parts:[Text("proto")]}, Text("_backend")]}]` |

#### 2. Hijacker Trait (hijack/mod.rs)

```rust
/* src/modules/template/hijack/mod.rs */

use anyhow::Result;
use async_trait::async_trait;

pub mod l7_http;
pub mod l7_dns; // future

/// Hijacker trait for layer-specific keyword handling
#[async_trait]
pub trait Hijacker: Send + Sync {
    /// Check if this hijacker handles the given key
    fn can_handle(&self, key: &str) -> bool;

    /// Resolve the hijack keyword
    async fn resolve(&self, key: &str) -> Result<String>;
}
```

**Design Rationale:**
- **Independent hijacking logic**: Separated from context implementation
- **Layer + Protocol organization**: Each file handles one layer+protocol combination
- **Extensibility**: Easy to add new protocol hijackers (DNS, gRPC, etc.)

#### 3. HTTP Hijacker (hijack/l7_http.rs)

```rust
/* src/modules/template/hijack/l7_http.rs */

use super::Hijacker;
use anyhow::Result;
use async_trait::async_trait;
use crate::modules::stack::protocol::application::container::Container;

/// HTTP-specific hijacker for L7 layer
pub struct HttpHijacker<'a> {
    pub container: &'a mut Container,
}

#[async_trait]
impl<'a> Hijacker for HttpHijacker<'a> {
    fn can_handle(&self, key: &str) -> bool {
        matches!(
            key,
            "req.body" | "req.body_hex" |
            "res.body" | "res.body_hex" |
            "req.headers" | "res.headers"
        ) || key.starts_with("req.header.")
          || key.starts_with("res.header.")
    }

    async fn resolve(&self, key: &str) -> Result<String> {
        // 1. Body hijacking (triggers lazy buffering)
        if key == "req.body" {
            let bytes = self.container.force_buffer_request().await?;
            return Ok(String::from_utf8_lossy(bytes).to_string());
        }

        if key == "req.body_hex" {
            let bytes = self.container.force_buffer_request().await?;
            return Ok(hex::encode(bytes));
        }

        if key == "res.body" {
            let bytes = self.container.force_buffer_response().await?;
            return Ok(String::from_utf8_lossy(bytes).to_string());
        }

        if key == "res.body_hex" {
            let bytes = self.container.force_buffer_response().await?;
            return Ok(hex::encode(bytes));
        }

        // 2. Header access
        if let Some(header_name) = key.strip_prefix("req.header.") {
            return Ok(get_header_value(&self.container.request_headers, header_name));
        }

        if let Some(header_name) = key.strip_prefix("res.header.") {
            return Ok(get_header_value(&self.container.response_headers, header_name));
        }

        if key == "req.headers" {
            return Ok(format!("{:?}", self.container.request_headers));
        }

        if key == "res.headers" {
            return Ok(format!("{:?}", self.container.response_headers));
        }

        Err(anyhow::anyhow!("Unsupported HTTP hijack key: {}", key))
    }
}

fn get_header_value(map: &http::HeaderMap, key_name: &str) -> String {
    match map.get(key_name) {
        Some(val) => val.to_str().unwrap_or("").to_string(),
        None => String::new(),
    }
}
```

#### 4. Context Trait (context.rs)

```rust
/* src/modules/template/context.rs */

use anyhow::Result;
use async_trait::async_trait;
use fancy_log::{LogLevel, log};
use crate::modules::kv::KvStore;
use crate::modules::stack::protocol::application::container::Container;
use super::hijack;

/// Template resolution context
#[async_trait]
pub trait TemplateContext: Send + Sync {
    /// Resolve a single key to string value
    /// Returns original template string ({{key}}) if not found
    async fn get(&self, key: &str) -> String;
}

/// L4/L4+ simple context (KV Store only)
pub struct SimpleContext<'a> {
    pub kv: &'a KvStore,
}

#[async_trait]
impl<'a> TemplateContext for SimpleContext<'a> {
    async fn get(&self, key: &str) -> String {
        match self.kv.get(key) {
            Some(value) => value.clone(),
            None => {
                log(
                    LogLevel::Warn,
                    &format!("⚠ Template key '{}' not found in KV Store, keeping original: {{{{{}}}}}", key, key)
                );
                format!("{{{{{}}}}}", key) // Return original {{key}}
            }
        }
    }
}

/// L7 context with hijacking support
pub struct L7Context<'a> {
    pub container: &'a mut Container,
}

#[async_trait]
impl<'a> TemplateContext for L7Context<'a> {
    async fn get(&self, key: &str) -> String {
        // 1. Try hijacking first (layer + protocol specific)
        let hijacker = hijack::l7_http::HttpHijacker {
            container: self.container,
        };

        if hijacker.can_handle(key) {
            match hijacker.resolve(key).await {
                Ok(value) => return value,
                Err(e) => {
                    log(
                        LogLevel::Warn,
                        &format!("⚠ Hijacking failed for '{}': {}, trying KV fallback", key, e)
                    );
                    // Fall through to KV Store
                }
            }
        }

        // 2. Fallback to KV Store
        match self.container.kv.get(key) {
            Some(value) => value.clone(),
            None => {
                log(
                    LogLevel::Warn,
                    &format!("⚠ Template key '{}' not found, keeping original: {{{{{}}}}}", key, key)
                );
                format!("{{{{{}}}}}", key) // Return original {{key}}
            }
        }
    }
}
```

**Design Rationale:**
- **Trait abstraction**: Each layer implements `TemplateContext` differently
- **L4/L4+ simplicity**: Only access KV Store
- **L7 hijacking**: Uses independent `HttpHijacker` from `hijack/l7_http.rs`
- **Error handling**: Returns original template string + warn log (no panic, no silent failure)
- **Extensibility**: Future protocols (DNS, gRPC) add new hijackers in `hijack/` directory

#### 5. Resolver (resolver.rs)

```rust
/* src/modules/template/resolver.rs */

use super::parser::TemplateNode;
use super::context::TemplateContext;

/// Resolve AST to final string
/// Never fails - returns original template string if key not found
pub async fn resolve_ast(
    nodes: &[TemplateNode],
    context: &dyn TemplateContext,
) -> String {
    let mut result = String::new();

    for node in nodes {
        match node {
            TemplateNode::Text(s) => {
                result.push_str(s);
            }
            TemplateNode::Variable { parts } => {
                // Recursively resolve nested parts
                let key = resolve_ast(parts, context).await;

                // Lookup in context (never fails, returns original on error)
                let value = context.get(&key).await;

                result.push_str(&value);
            }
        }
    }

    result
}
```

**Resolution Process:**
1. Traverse AST nodes sequentially
2. For `Text` nodes: append directly
3. For `Variable` nodes:
   - Recursively resolve `parts` (handles nesting and concatenation)
   - Use resolved string as key
   - Call `context.get(key)` (may trigger hijacking in L7, returns original on error)
   - Append result

**Error Handling:**
- No `Result` return type - never fails
- Missing keys return original template string (e.g., `{{missing}}`)
- Warn logs printed by `TemplateContext` implementation

#### 6. Public API (mod.rs)

```rust
/* src/modules/template/mod.rs */

pub mod parser;
pub mod resolver;
pub mod context;
pub mod hijack;

pub use context::TemplateContext;

use anyhow::Result;
use serde_json::{Map, Value};
use std::collections::HashMap;

/// High-level API: Parse and resolve template string
/// Returns original string on parse error (with log)
pub async fn resolve_template(
    template: &str,
    context: &dyn TemplateContext,
) -> String {
    match parser::parse_template(template) {
        Ok(ast) => resolver::resolve_ast(&ast, context).await,
        Err(e) => {
            fancy_log::log(
                fancy_log::LogLevel::Warn,
                &format!("⚠ Template parse error: {}, returning original string", e)
            );
            template.to_string()
        }
    }
}

/// Helper for resolving plugin inputs (HashMap<String, Value>)
/// Never fails - returns original values on error
pub async fn resolve_inputs(
    inputs: &HashMap<String, Value>,
    context: &dyn TemplateContext,
) -> HashMap<String, Value> {
    let mut resolved = HashMap::new();

    for (key, value) in inputs {
        let resolved_val = resolve_value_recursive(value, context).await;
        resolved.insert(key.clone(), resolved_val);
    }

    resolved
}

/// Recursive helper for JSON structures (Arrays, Objects)
async fn resolve_value_recursive(
    value: &Value,
    context: &dyn TemplateContext,
) -> Value {
    match value {
        Value::String(s) => {
            let result = resolve_template(s, context).await;
            Value::String(result)
        }
        Value::Array(arr) => {
            let mut new_arr = Vec::with_capacity(arr.len());
            for item in arr {
                new_arr.push(resolve_value_recursive(item, context).await);
            }
            Value::Array(new_arr)
        }
        Value::Object(map) => {
            let mut new_map = Map::with_capacity(map.len());
            for (k, v) in map {
                new_map.insert(k.clone(), resolve_value_recursive(v, context).await);
            }
            Value::Object(new_map)
        }
        _ => value.clone(), // Numbers, Bools, Nulls are kept as-is
    }
}
```

**API Design:**
- **No Result return**: All functions return success values
- **Graceful degradation**: Parse errors or missing keys return original string
- **Logging**: All errors logged at Warn level via `fancy_log`
- **Simple usage**: Callers don't need to handle errors

---

## Integration with Existing Code

### L4 Layer (transport/flow.rs)

**Before:**
```rust
fn resolve_inputs(
    inputs: &HashMap<String, Value>,
    kv: &KvStore,
) -> HashMap<String, Value> {
    // ~20 lines of inline code
}
```

**After:**
```rust
use crate::modules::template::{resolve_inputs, context::SimpleContext};

// In execute_flow()
let context = SimpleContext { kv };
let resolved_inputs = resolve_inputs(&instance.input, &context).await; // No Result!
```

**Changes:**
- Delete inline `resolve_inputs` function
- Use unified template module
- No error handling needed (never fails)

### L4+ Layer (carrier/flow.rs)

**Same as L4** - delete duplicate code, use unified template module.

### L7 Layer (application/flow.rs)

**Before:**
```rust
use super::template::resolve_inputs;

// In execute_flow()
let resolved_inputs = resolve_inputs(&instance.input, container).await?;
```

**After:**
```rust
use crate::modules::template::{resolve_inputs, context::L7Context};

// In execute_flow()
let mut context = L7Context { container };
let resolved_inputs = resolve_inputs(&instance.input, &mut context).await; // No Result!
```

**Changes:**
- Replace `super::template` with unified `crate::modules::template`
- Use `L7Context` wrapper (supports hijacking)
- No error handling needed (never fails)

**Delete:** `src/modules/stack/protocol/application/template.rs` (replaced by unified module)

---

## Edge Cases to Handle

- **Empty templates**: `""` → `""`
- **No variables**: `"plain text"` → `"plain text"`
- **Adjacent variables**: `"{{a}}{{b}}"` → concatenate both
- **Missing keys**: Return original template string `{{key}}` + warn log
- **Circular references**: May cause infinite recursion - need depth limit in resolver
  - Example: `{{kv.{{kv.x}}}}` where `kv.x = "{{kv.x}}"` → infinite loop
  - Solution: Add `max_depth` parameter (default: 10)
- **Escape sequences**: Not implemented in initial version
  - Users who need literal `{{` can use KV: `kv.set("lbrace", "{{")`
  - Can be added later if there's demand

---

## Testing Strategy

### Unit Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// Tests simple variable replacement.
    #[tokio::test]
    async fn test_simple_replacement() {
        let mut kv = KvStore::new();
        kv.set("key", "value");

        let context = SimpleContext { kv: &kv };
        let result = resolve_template("{{key}}", &context).await;

        assert_eq!(result, "value");
    }

    /// Tests string concatenation with multiple variables.
    #[tokio::test]
    async fn test_concatenation() {
        let mut kv = KvStore::new();
        kv.set("conn.ip", "1.2.3.4");
        kv.set("conn.port", "8080");

        let context = SimpleContext { kv: &kv };
        let result = resolve_template("{{conn.ip}}:{{conn.port}}", &context).await;

        assert_eq!(result, "1.2.3.4:8080");
    }

    /// Tests nested template resolution.
    #[tokio::test]
    async fn test_nested() {
        let mut kv = KvStore::new();
        kv.set("conn.protocol", "http");
        kv.set("kv.http_backend", "backend-01");

        let context = SimpleContext { kv: &kv };
        let result = resolve_template("{{kv.{{conn.protocol}}_backend}}", &context).await;

        assert_eq!(result, "backend-01");
    }

    /// Tests complex nested template with concatenation.
    #[tokio::test]
    async fn test_complex() {
        let mut kv = KvStore::new();
        kv.set("geo.country", "US");
        kv.set("kv.US_domain", "api.example.com");

        let context = SimpleContext { kv: &kv };
        let result = resolve_template("https://{{kv.{{geo.country}}_domain}}/api", &context).await;

        assert_eq!(result, "https://api.example.com/api");
    }

    /// Tests that missing keys return original template string.
    #[tokio::test]
    async fn test_missing_key() {
        let kv = KvStore::new();
        let context = SimpleContext { kv: &kv };
        let result = resolve_template("{{missing}}", &context).await;

        // Should return original template, not error
        assert_eq!(result, "{{missing}}");
    }

    /// Tests that empty templates work correctly.
    #[tokio::test]
    async fn test_empty_template() {
        let kv = KvStore::new();
        let context = SimpleContext { kv: &kv };
        let result = resolve_template("", &context).await;

        assert_eq!(result, "");
    }

    /// Tests that plain text without variables is unchanged.
    #[tokio::test]
    async fn test_plain_text() {
        let kv = KvStore::new();
        let context = SimpleContext { kv: &kv };
        let result = resolve_template("plain text", &context).await;

        assert_eq!(result, "plain text");
    }
}
```

### Integration Tests

Add Go tests in `integration/tests/l4/`, `l4p/`, `l7/` to verify:
- String concatenation works in configurations
- Nested templates resolve correctly
- L7 hijacking still triggers buffering
- Error messages are clear for missing keys

---

## Migration Plan

### Phase 1: Create Unified Module (2-3 days)
- [ ] Create `src/modules/template/` directory structure
- [ ] Implement `parser.rs` (Lexer + Parser for AST)
  - [ ] Tokenizer for `{{`, `}}`, and text
  - [ ] Recursive parser for nested structures
  - [ ] Add tests for edge cases (empty, nested, concatenation)
- [ ] Implement `hijack/mod.rs` (Hijacker trait)
- [ ] Implement `hijack/l7_http.rs` (HTTP hijacking logic)
  - [ ] Body hijacking (`req.body`, `res.body`, hex variants)
  - [ ] Header access (`req.header.*`, `res.header.*`)
  - [ ] Add tests for all HTTP keywords
- [ ] Implement `context.rs` (TemplateContext trait + SimpleContext + L7Context)
  - [ ] SimpleContext for L4/L4+
  - [ ] L7Context using HttpHijacker
  - [ ] Error handling (return original + warn log)
- [ ] Implement `resolver.rs` (AST evaluation)
  - [ ] Recursive resolution
  - [ ] Depth limit to prevent infinite loops
  - [ ] Add tests for complex nesting
- [ ] Implement `mod.rs` (Public API)
  - [ ] `resolve_template()` function
  - [ ] `resolve_inputs()` for JSON structures
  - [ ] Add integration tests

### Phase 2: Migrate L4 and L4+ (1 day)
- [ ] Update `transport/flow.rs`: Replace inline function with template module
- [ ] Update `carrier/flow.rs`: Replace inline function with template module
- [ ] Delete duplicate code
- [ ] Run `cargo test` to verify no regressions

### Phase 3: Migrate L7 (1 day)
- [ ] Update `application/flow.rs`: Use unified template module with L7Context
- [ ] Delete `application/template.rs` (replaced by unified module)
- [ ] Verify hijacking still works (test `req.body` access)
- [ ] Run `cargo test` to verify no regressions

### Phase 4: Integration Testing (1 day)
- [ ] Add Go integration tests for string concatenation
- [ ] Add Go integration tests for nested templates
- [ ] Verify L7 hijacking behavior unchanged
- [ ] Run full test suite

### Phase 5: Documentation (0.5 day)
- [ ] Update `docs/core-modules/stack.md` with new template features
- [ ] Update `docs/configuration.md` with concatenation/nesting examples
- [ ] Update `CHANGELOG.md` with template system improvements

---

## Benefits

✅ **Eliminates code duplication** (~40 lines removed across L4/L4+)
✅ **Unified maintenance** - single source of truth for template logic
✅ **Feature parity** - all layers support concatenation and nesting
✅ **More expressive configuration** - no workarounds needed
✅ **Hijacking logic isolated** - easy to add new protocols (DNS, gRPC, etc.)
✅ **Clean architecture** - separation of concerns (parser, resolver, context, hijack)
✅ **Extensibility** - trait-based design allows custom contexts
✅ **Backward compatible** - simple `{{key}}` templates still work
✅ **Graceful error handling** - returns original template on error, no panics

---

## Impact

- **Code reduction**: ~160 lines of total code removed (40 lines L4/L4+ duplication + 120 lines old L7 template.rs)
- **Code addition**: ~400 lines of new unified template module (with better architecture)
- **Net change**: +240 lines, but with much better organization and features
- **Feature addition**: String concatenation, nested templates, isolated hijacking
- **Architecture improvement**: Clean trait-based abstraction, separated concerns
- **No breaking changes**: Existing simple templates continue to work
- **Error handling improvement**: No more Result propagation, graceful degradation

---

## Complexity

**Medium** (parser design + careful testing for edge cases)

---

## Estimated Time

**5-6 days total** (including implementation, testing, and documentation)
