# Task 2.21: Resolve CGI PATH_INFO Edge Cases

**Goal:** Correct the PATH_INFO and SCRIPT_NAME derivation logic in the CGI driver to strictly adhere to RFC 3875 and handle various URI patterns reliably.

## Current Issues

### 1. Lack of Path Segment Boundaries
The current code uses `starts_with(script_name)` to calculate `path_info`.
- **Example:** `script_name = "/cgi"`, `uri = "/cgi-bin/script"`.
- **Result:** `path_info = "-bin/script"` (Incorrect).
- **Correct Behavior:** Should only strip if `script_name` matches a full path segment or multiple segments.

### 2. Missing Default Logic
If `script_name` is not provided in the configuration, it defaults to an empty string, which disables the automatic `PATH_INFO` calculation.
- **Problem:** Users have to manually specify `script_name` for every CGI instance even if it's predictable from the routing.

### 3. Redundant Slashes
The logic doesn't normalize double slashes (e.g., `//cgi-bin//script`).

---

## Proposed Design

### 1. Robust Derivation Function
Implement a utility function that safely splits a URI path into `SCRIPT_NAME` and `PATH_INFO` given a base script path.

```rust
fn derive_cgi_paths(full_path: &str, script_base: &str) -> (String, String) {
    // 1. Normalize slashes
    // 2. Find script_base in full_path at segment boundary
    // 3. Split
}
```

### 2. Enhanced Configuration Options
- **`script_name` (optional)**: If provided, use it as the split point.
- **`auto_derive` (new flag, default true)**: Attempt to guess `SCRIPT_NAME` based on the `script` (file path) or the request path.

### 3. RFC 3875 Compliance
- Ensure `PATH_INFO` always starts with a `/` if not empty.
- Ensure `SCRIPT_NAME` does NOT end with a `/` unless it's the root.

---

## Implementation Plan

- [ ] Create unit tests in `src/modules/plugins/cgi/plugin.rs` covering edge cases:
    - URI equals SCRIPT_NAME.
    - URI starts with SCRIPT_NAME but no segment match.
    - SCRIPT_NAME is `/`.
    - Empty SCRIPT_NAME.
- [ ] Refactor `plugin.rs` logic to use the new derivation rules.
- [ ] Update `executor.rs` to ensure `PATH_TRANSLATED` is correctly handled.
- [ ] Verify with integration tests (Go).

---

## Discussion
- Should Vane attempt to auto-detect `SCRIPT_NAME` by checking the filesystem? (Probably too slow for data plane).
- Should we rely on the `uri` provided in the plugin input (which might be a template like `{{req.path}}`)?
