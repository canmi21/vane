# Analysis: Code Organization & Structure

**Date:** 2026-01-02
**Context:** Phase IV Deep Analysis

## 1. Folder Structure Improvements

### 1.1 `src/common` Cleanup
The `common` directory has become a "catch-all".
-   **Proposal:** Split `requirements.rs` into:
    -   `src/common/watcher.rs` (Config watching logic)
    -   `src/common/lifecycle.rs` (Startup/Shutdown logic)
-   **Proposal:** Move `portool.rs` and `ip.rs` to `src/common/net/`.

### 1.2 `src/core` Refactor
`bootstrap.rs` is too large.
-   **Proposal:** Extract console server logic to `src/core/console.rs`.
-   **Proposal:** Extract logging setup to `src/core/logging.rs`.

### 1.3 `src/modules/stack/transport`
Legacy code still exists.
-   **Proposal:** Remove `legacy` folder and merge essential logic into `tcp.rs`/`udp.rs` or delete if fully superseded by Flow.
-   **Proposal:** Split `proxy.rs` into `proxy/tcp.rs` and `proxy/udp.rs`.

## 2. File Naming & Keyword Avoidance

-   **Current:** `src/modules/plugins/l7/resource/static.rs`
    -   *Issue:* `static` is a keyword. Currently using `r#static` in imports.
    -   *Fix:* Rename to `file_server.rs` or `assets.rs`.
-   **Current:** `src/modules/plugins/middleware/match.rs` (Hypothetical, currently `matcher.rs`)
    -   *Check:* Ensure no files are named `match.rs`, `type.rs`, `loop.rs`.
-   **Current:** `src/modules/plugins/core/model.rs`
    -   *Improvement:* Rename to `types.rs` or `definitions.rs` to be more descriptive? (Low priority).

## 3. Dependency Injection
-   **Current:** `GLOBAL_TRACKER`, `CONFIG_STATE` are static globals.
-   **Improvement:** Pass state via `Arc<State>` context where possible to improve testability, though global state is acceptable for top-level config.
