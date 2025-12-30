# Task 0.2.2: Plugin System Refactoring

**Status**: Planned

**Goal**: Refactor the plugin system to enforce clear boundaries between Generic and Protocol-Specific middleware, ensuring type safety and correct usage at config load time.

**Dependencies**:
- Task 1.2: Flow engine extraction (✅ Completed)
- Task 1.3: Hot-reload framework (✅ Completed)

---

## 1. Core Concept: Two-Tier Middleware

### Generic Middleware (Universal)
- **Scope**: Cross-layer (L4, L4+, L7).
- **Inputs**: Strictly limited to `ResolvedInputs` (JSON-compatible, via `{{template}}`).
- **Outputs**: `MiddlewareOutput` (branch + KV updates).
- **Capabilities**:
  - NO direct access to `Container` or `Socket`.
  - NO handling of streams/body.
  - Can be **Internal** (Rust) or **External** (HTTP/Unix/Command).
- **Execution**: Flow Engine handles KV writes based on output.
- **Examples**: `CommonMatch`, `RateLimit`, External Webhooks.

### Protocol-Specific Middleware (Layer-Bound)
- **Scope**: Bound to specific layers/protocols (e.g., L7+HTTP, L4+QUIC).
- **Inputs**: `ResolvedInputs` + **Direct Context Access**.
- **Outputs**: `MiddlewareOutput`.
- **Capabilities**:
  - Full read/write access to `Container` (L7) or `TcpStream` (L4).
  - Can handle zero-copy streams (Body, Upgrades).
  - MUST be **Internal** (Rust only).
- **Examples**: `FetchUpstream` (HTTP), `StaticFile` (HTTP), `SniRouting` (TLS).

---

## 2. Implementation Phases

### Phase 1: Core Definitions & Categorization (3-4 hours)

**Goal**: Define the new Trait hierarchy and categorize existing plugins.

**Changes**:
1.  **Modify `src/modules/plugins/model.rs`**:
    -   Add `GenericMiddleware` trait (replaces generic usage of `Middleware`).
    -   Add `HttpMiddleware` trait (takes `&mut Container`).
    -   Update `Plugin` trait to support `as_generic()` and `as_http()`.
    -   Add `supported_protocols()` metadata to `Plugin` trait.
2.  **Update `src/modules/plugins/registry.rs`**:
    -   Categorize all built-in plugins.
    -   Generic: `ProtocolDetect`, `CommonMatch`, `RateLimit`.
    -   HttpSpecific: `FetchUpstream`, `Cgi`, `Static`, `SendResponse`.

### Phase 2: Enhanced Validation & Loading (4-5 hours)

**Goal**: Enforce compatibility checks at config load time.

**Changes**:
1.  **Update Validator (`src/modules/stack/transport/validator.rs`)**:
    -   Update `validate_flow_config` to accept a `protocol` context (e.g., "httpx", "tcp").
    -   **Logic**:
        -   If plugin is `HttpSpecific`: Check if `current_protocol` is in its `supported_protocols`. If not -> Error (Fail Fast).
        -   If plugin is `Generic`: Always allowed.
2.  **Update External Loader**:
    -   Keep startup "skip validation" logic (to prevent boot blocking).
    -   **New Feature**: Implement a **Background Health Checker Task** for external plugins.
        -   Runs every `EXTERNAL_PLUGIN_CHECK_INTERVAL` (default: 15m).
        -   Updates an in-memory status table (for future API/Dashboard).
        -   Does NOT disable the plugin (runtime failure handled by "failure" branch).

### Phase 3: Flow Engine & Runtime (4-5 hours)

**Goal**: Update execution logic and ensure robust error handling.

**Changes**:
1.  **Update Flow Engine (`src/modules/flow/engine.rs`)**:
    -   Refactor dispatch logic to try `HttpMiddleware` first (if context allows), then `GenericMiddleware`.
2.  **External Driver Error Handling**:
    -   Verify `src/modules/plugins/drivers/` implementations.
    -   Ensure IO/Network errors return `MiddlewareOutput { branch: "failure" }` instead of `Result::Err`.
    -   This allows users to configure `failure` branch behavior (abort, log, fallback).

---

## 3. Configuration & Validation Logic

**Load Time (Strict):**
- "I am loading `httpx` config."
- "Flow uses `fetch_upstream`." -> Is `fetch_upstream` HTTP-compatible? Yes. -> OK.
- "Flow uses `sni_routing`." -> Is `sni_routing` HTTP-compatible? No (TLS only). -> **Error: Config Rejected.**

**Run Time (Resilient):**
- "Executing `external_auth` (Generic)."
- "External service down?" -> Return `branch: "failure"`.
- "Flow config says `failure` -> `abort`." -> Connection closed cleanly.

---

## 4. Dependencies

- `src/common/loader.rs` (Used for config loading)
- `src/common/hotswap.rs` (Used for config watching)