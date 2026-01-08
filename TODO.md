# Vane Documentation TODO

This document tracks the comprehensive documentation plan for Vane. Documentation is organized into three main sections: User Documentation, Development Documentation, and Contributing Guidelines.

## Documentation Structure Overview

```text
docs/
├── (user)/              # User-facing documentation (fumadocs group syntax)
├── development/         # Developer implementation documentation
└── contributing/        # Documentation writing and contribution guidelines
```

## Development Documentation Implementation Plan

This plan outlines the sequence for filling out the development documentation skeleton. It follows a logical progression from system startup to core engine mechanics, and finally to specific layer implementations.

### Phase 1: Foundation & Startup

- [x] `docs/development/bootstrap/index.mdx`
- [x] `docs/development/bootstrap/startup-sequence.mdx`
- [x] `docs/development/bootstrap/monitoring.mdx`
- [x] `docs/development/common/index.mdx`
- [x] `docs/development/common/configuration.mdx`
- [x] `docs/development/common/network.mdx`
- [x] `docs/development/common/system.mdx`

### Phase 2: Core Resources & Ingress

- [x] `docs/development/resources/index.mdx`
- [x] `docs/development/resources/kv-store.mdx`
- [x] `docs/development/resources/templates.mdx`
- [x] `docs/development/resources/service-discovery.mdx`
- [x] `docs/development/resources/certificates.mdx`
- [ ] `docs/development/ingress/index.mdx`
- [ ] `docs/development/ingress/listeners.mdx`
- [ ] `docs/development/ingress/connection-management.mdx`

### Phase 3: The Engine & Plugin System

- [ ] `docs/development/engine/index.mdx`
- [ ] `docs/development/engine/flow-executor.mdx`
- [ ] `docs/development/engine/plugin-system.mdx`
- [ ] `docs/development/plugins/index.mdx`
- [ ] `docs/development/plugins/architecture.mdx`
- [ ] `docs/development/plugins/types.mdx`

### Phase 4: Network Layers

- [ ] `docs/development/layers/index.mdx`
- [ ] **Layer 4**
  - [ ] `docs/development/layers/l4/index.mdx`
  - [ ] `docs/development/layers/l4/flow-routing.mdx`
  - [ ] `docs/development/layers/l4/proxy.mdx`
  - [ ] `docs/development/layers/l4/session-management.mdx`
- [ ] **Layer 4+**
  - [ ] `docs/development/layers/l4p/index.mdx`
  - [ ] `docs/development/layers/l4p/flow-management.mdx`
  - [ ] `docs/development/layers/l4p/protocols.mdx`
- [ ] **Layer 7**
  - [ ] `docs/development/layers/l7/index.mdx`

### Phase 5: API & Wrap-up

- [ ] `docs/development/api/index.mdx`
- [ ] `docs/development/api/management-api.mdx`
- [ ] `docs/development/index.mdx` (Root Overview)
- [ ] `docs/development/roadmap.mdx`
