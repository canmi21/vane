# Priority 2: Performance & Usability Tasks

These tasks are deferred until Phase I and Phase II are complete.

## Task 2.1: External Plugin Connection Pooling

**Status:** Planned
**Description:** Reduce external plugin latency by reusing HTTP/Unix connections

**Benefits:** 5x latency reduction (5ms → 1ms for localhost)
**Complexity:** Low (reqwest handles pooling)

---

## Task 2.2: Template Function System

**Status:** Planned
**Description:** Extend templates with function syntax (e.g., `{{key|default:value}}`, `{{text|hash:sha256}}`)

**Benefits:** More expressive configuration, no custom plugins for simple transformations
**Complexity:** Medium (parser extension)

---

## Task 2.3: Streaming Template Access

**Status:** Planned
**Description:** Allow partial payload access without full buffering (e.g., `{{req.body.peek:1024}}`)

**Benefits:** Inspect large payloads without OOM
**Complexity:** High (stream manipulation)
