# Task 1.1: Rust Feature Flags Support

**Status:** Planned (Phase II)

**User Input:** 支持这么多协议会导致 binary 过大，需要 feature 来按需编译

**Blocker:** Awaiting discussion on feature granularity

## Proposed Feature Flags

### Transport Layer (L4)

```toml
[features]
default = ["tcp", "udp"]
tcp = []
udp = []
```

### Carrier Layer (L4+)

```toml
tls = ["dep:rustls", "dep:tokio-rustls"]
quic = ["dep:quinn", "dep:h3"]
dtls = ["tls"]  # Future
```

### Application Layer (L7)

```toml
http = ["http1", "http2"]
http1 = ["dep:hyper"]
http2 = ["dep:hyper"]
http3 = ["quic", "dep:h3"]
dns = []  # Future
```

### Plugin Categories

```toml
# Middleware
middleware-protocol-detect = []
middleware-ratelimit = []
middleware-matcher = []

# Terminators
terminator-proxy = []
terminator-upgrade = []

# L7 Drivers
driver-fetch-upstream = ["http"]
driver-cgi = []
driver-static = []
```

### External Plugin System

```toml
external-plugins = []
external-http-driver = ["external-plugins"]
external-unix-driver = ["external-plugins"]
external-command-driver = ["external-plugins"]
```

## Discussion Points

1. 粒度如何控制？每个插件一个 feature？还是按类别（middleware, terminator, driver）？
2. 默认启用哪些 features？是否提供预设组合（minimal, standard, full）？
3. 是否需要 feature 互斥检查（例如 http1 和 http2 不能同时禁用）？
4. 如何测试所有 feature 组合？（组合爆炸问题）

## Implementation Plan

- [ ] Design feature flag hierarchy
- [ ] Add feature flags to Cargo.toml
- [ ] Add #[cfg(feature = "...")] to relevant code
- [ ] Update build.rs if needed (conditional compilation)
- [ ] Test: Binary size reduction for minimal feature set
- [ ] Document: README with feature flag usage examples
- [ ] CI: Test multiple feature combinations

## Benefits

- Smaller binary size for single-protocol deployments
- Faster compilation (fewer dependencies)
- Clearer module boundaries (dependencies explicit)

## Complexity

Medium (requires careful dependency management)
