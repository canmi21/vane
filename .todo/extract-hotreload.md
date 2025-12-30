# Task 1.3: Extract Hot-Reload Framework

**Status:** Planned (Phase I)

**Description:** Eliminate duplication across nodes, ports, certs, and application hotswap logic (~300 lines) by extracting common loader and watcher patterns.

## Analysis

Existing modules (`nodes`, `ports`, `certs`, `application`) share two core responsibilities:
1. **Loading**: Finding config files with various extensions, handling conflicts, parsing, and validating.
2. **Watching**: Listening to `mpsc::Receiver<()>`, logging changes, reloading, and updating global state.

Currently, `src/modules/stack/transport/loader.rs` already contains excellent generic logic for step 1, but it's buried in the L4 transport module.

## detailed Design

### 1. Unified Loader (`src/common/loader.rs`)

Move `src/modules/stack/transport/loader.rs` to `src/common/loader.rs` to make it a first-class citizen.

```rust
pub trait PreProcess {
    fn pre_process(&mut self) {} // Default no-op
}

/// Loads a specific file (e.g., "app.json")
pub fn load_file<T>(path: &Path) -> Option<T>;

/// Scans for "base.{ext}" (e.g., "nodes.yaml", "nodes.json") and handles conflicts
pub fn load_config<T>(base_name: &str, base_path: &Path) -> Option<T>;
```

### 2. Unified Watcher Loop (`src/common/hotswap.rs`)

Extract the boilerplate loop used in all `listen_for_updates` functions.

```rust
pub async fn watch_loop<F, Fut>(
    mut rx: mpsc::Receiver<()>,
    name: &str,
    mut on_reload: F
)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = ()>,
{
    while rx.recv().await.is_some() {
        log(LogLevel::Info, &format!("➜ {} config changed, reloading...", name));
        on_reload().await;
    }
}
```

## Implementation Phases

### Phase 1: Core Extraction
- Move `src/modules/stack/transport/loader.rs` -> `src/common/loader.rs`.
- Create `src/common/hotswap.rs`.
- Verify compilation.

### Phase 2: Migrate Nodes & Application
- Refactor `src/modules/nodes/hotswap.rs` to use common loader/watcher.
- Refactor `src/modules/stack/protocol/application/hotswap.rs` to use common loader/watcher.

### Phase 3: Migrate Ports & Certs
- Refactor `src/modules/ports/hotswap.rs` (slightly more complex logic).
- Refactor `src/modules/certs/loader.rs` (logic is embedded in loader, separate it).

## Benefits
- **DRY**: Eliminates ~300 lines of duplicated IO/Loop code.
- **Consistency**: All modules behave exactly the same regarding file conflicts and extensions.
- **Maintainability**: Centralized error handling and logging for configuration loading.