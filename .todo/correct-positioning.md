# Task 0.1: Correct Vane Positioning in Documentation

**Status:** ✅ Completed (2025-12-29)

**User Input:** Vane 不是"反向代理"，是"网络协议引擎"，HTTP 只是众多协议之一

## Completed Changes

- ✅ README.md: Updated to "network protocol engine", added protocol funnel description
- ✅ CLAUDE.md: Updated project identity, emphasized multi-protocol support
- ✅ docs/overview.md: Changed "reverse proxy" to "network protocol engine"
- ⏳ ARCHITECTURE.md: Will update after Phase I implementation (not before)
- ⏳ CODE.md: Will update after Phase I implementation (not before)

## Key Messaging

- Vane is a "network protocol engine" (not "reverse proxy")
- Operates as a "protocol funnel" (L4 → L4+ → L7)
- HTTP is one of many supported protocols
- Designed for extensibility to DNS, gRPC, and other application protocols

## Impact

Documentation now correctly represents Vane's scope and capabilities

## Complexity

Very Low (documentation updates only)
