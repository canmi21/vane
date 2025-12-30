# Task 1.3: Extract Hot-Reload Framework

**Status:** Planned (Phase I)

**Description:** Eliminate duplication across nodes/hotswap.rs, ports/hotswap.rs, certs/hotswap.rs (~300 lines duplicated)

## Proposed Design

```rust
// common/hotswap.rs
pub trait HotSwappable: Sized {
    type Config;
    fn load(path: &str) -> Result<Self::Config>;
    fn validate(config: &Self::Config) -> Result<()>;
}

pub fn watch_and_reload<T: HotSwappable>(
    path: &str,
    registry: ArcSwap<T::Config>,
) -> Result<()> {
    // Generic watch-and-reload logic
}
```

## Benefits

- No code duplication
- Consistent hot-reload behavior
- Easy to add new hot-swappable components

## Complexity

Low (straightforward trait extraction)

## Estimated Time

2-3 days
