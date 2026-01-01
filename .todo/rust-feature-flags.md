# Task 1.1: Rust Feature Flags Support - Deep Investigation

**Goal:** Implement a fine-grained feature flag system to allow custom, lightweight builds of Vane while keeping the default build "full-featured".

## Status: Investigation Phase Complete

### 1. Dependency Analysis & Impacts

| Category | Key Dependencies | Complexity | Impact on Binary Size |
|----------|------------------|------------|-----------------------|
| **QUIC/H3** | `quinn`, `h3`, `h3-quinn` | High | Very High |
| **TLS** | `rustls`, `tokio-rustls`, `rcgen` | High | High |
| **L7 HTTP** | `hyper`, `hyper-util`, `axum` | Medium | High |
| **DNS** | `hickory-resolver` | Low | Medium |
| **Ext HTTP** | `reqwest` | Low | Medium |

---

### 2. Feature Architecture Design

#### Master Flags
```toml
[features]
default = ["full"]
full = [
    "tls", "quic", "http", "http3", "dns", 
    "plugins-full", "external-full"
]
```

#### Detailed Flags
- **Protocols:** `tls`, `quic`, `http`, `http3` (requires `quic`), `dns`.
- **Built-in Plugins:** `plugin-cgi`, `plugin-static`, `plugin-upstream`, `plugin-ratelimit`.
- **External Drivers:** `external-http` (gates `reqwest`), `external-unix`, `external-exec`.

---

### 3. Gradual Roll-out Roadmap

We will add flags one by one to ensure stability.

#### Step 1: Built-in Plugin Gating (Phase 2 in Roadmap)
- [ ] Add `plugin-cgi` flag. Gate `CgiPlugin` registration and its module.
- [ ] Add `plugin-static` flag. Gate `StaticPlugin` registration and its module.
- [ ] Add `plugin-ratelimit` flag. Gate `KeywordRateLimit*` registration.

#### Step 2: External Driver Gating (Phase 3 in Roadmap)
- [ ] Add `external-http` flag. Gate `reqwest` and `drivers::httpx`.
- [ ] Add `external-exec` flag. Gate `drivers::exec`.
- [ ] Add `external-unix` flag. Gate `drivers::unix`.

#### Step 3: Core Protocol Gating (Phase 4 in Roadmap)
- [ ] Add `dns` flag. Gate `hickory-resolver` and `proxy::domain`.
- [ ] Add `quic` flag. Gate `quinn` and `carrier::quic`. Update `udp.rs` dispatcher.
- [ ] Add `tls` flag. Gate `rustls` and `carrier::tls`. Update `tcp.rs` dispatcher.
- [ ] Add `http` flag. Gate `hyper` and `application::http::httpx`.
- [ ] Add `http3` flag. Gate `h3` and `application::http::h3`.

---

### 4. Implementation Guidelines

1. **Imports:** Always gate imports corresponding to the feature.
2. **Registry:** Use `#[cfg(feature = "...")]` inside `Lazy` blocks.
3. **Dispatcher:** If a feature is disabled, the corresponding protocol upgrade arm should return a `LogLevel::Error` log or be blocked by the `validator`.
4. **Validator:** The `validate_flow_config` should be updated to check if the plugin/protocol is actually compiled in.

---

## Next Steps
1. Present this design to the user for approval.
2. Start with **Step 1: Built-in Plugin Gating (plugin-cgi)**.