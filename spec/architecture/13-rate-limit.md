# Rate Limit and DoS Protection

Two-layer model. L1 is non-negotiable daemon self-preservation; L2 is opt-in application-layer policy expressed as middleware.

```
Incoming traffic
  ↓
[L1: Security floor]  — always on, cannot be disabled, values configurable but bounded below
  ↓ pass
[L2: User rate limit] — opt-in per-rule middleware
  ↓ pass
[FlowGraph dispatch]
```

---

## L1 — Daemon self-preservation

Enforced at **listener accept / pre-handshake / header-parse** boundaries, before any FlowGraph walk. Architecturally positioned outside user rules.

### Threat coverage

| Threat                                                  | Mechanism                                              |
| ------------------------------------------------------- | ------------------------------------------------------ |
| Slowloris (slow-read connections exhaust FDs)           | header and body timeouts                               |
| Slow POST (slow-write body exhausts memory)             | body idle timeout                                      |
| Connection flood (single source floods conns)           | per-IP concurrent cap                                  |
| Handshake flood (TLS CPU exhaustion)                    | per-IP handshake rate + global pending cap             |
| Header flood (oversized headers exhaust memory)         | max header size + count                                |
| H2/H3 stream flood (multiplexed requests exhaust tasks) | per-conn stream cap + per-conn request rate            |
| WASM call flood                                         | covered separately by WASM pool cap (see `11-wasm.md`) |

### Limits

| Limit                            | Default  | Minimum floor | Layer         | Trigger behavior   |
| -------------------------------- | -------- | ------------- | ------------- | ------------------ |
| `max_header_bytes`               | 64 KiB   | 4 KiB         | L7 parse      | close + 400        |
| `max_headers_count`              | 100      | 20            | L7 parse      | close + 400        |
| `header_timeout`                 | 30 s     | 5 s           | ingress       | close              |
| `body_idle_timeout`              | 30 s     | 5 s           | L7 body       | close + 408        |
| `max_concurrent_conns_per_ip`    | 100      | 10            | accept        | reject new conn    |
| `max_handshake_rate_per_ip`      | 10 / s   | 1 / s         | pre-handshake | TCP reset          |
| `max_in_flight_streams_per_conn` | 100      | 10            | H2 / H3       | RST_STREAM         |
| `max_request_rate_per_conn`      | 1000 / s | 100 / s       | H2 / H3       | RST_STREAM / close |
| `max_total_connections`          | 65 536   | 1 024         | accept        | reject new conn    |
| `max_pending_handshakes`         | 1 000    | 100           | TLS accept    | reject new conn    |

### Key semantics

- **Always present**: no `disabled: true` option anywhere. Every limit has a numeric value at all times.
- **Adjustable upward**: config can raise any limit (for high-traffic production).
- **Floor enforced at compile**: setting a value below the floor is a compile error with a message explaining the minimum. `vane compile --dry-run` catches misconfiguration before it reaches a running daemon.
- **Default calibration target**: normal traffic (moderate home / small-business / medium commercial sites with a mix of human and bot visitors) should never trigger any of these. Triggering means either a misbehaving client or a deliberate attack.

### Observability under attack (preventing log amplification)

A flood can produce millions of limit-hits per second. Naive logging amplifies the attack by drowning disk I/O and log pipes.

- **Structured log**: at `warn` level, dedup by `(limit, source_ip)` within a 1-second window. One line per attack path per second.
- **Flow log**: emit `event: "security_limit"` with the same dedup semantics.
- **Metric**: `vane.security.limit_hit_total{limit, source}` counter — no dedup, full fidelity (metrics are designed to absorb high-cardinality counting).

### Configuration location

L1 limits are **deployment-level constants**, configured via environment variables loaded by `dotenvy` at startup:

```
VANE_SEC_MAX_HEADER_BYTES=65536
VANE_SEC_MAX_HEADERS_COUNT=100
VANE_SEC_HEADER_TIMEOUT=30
VANE_SEC_MAX_CONN_PER_IP=100
# ...
```

Changing L1 limits requires a daemon restart. They are not in `config.json` or `rules/*.json`. This matches their role: they are about the daemon's existence, not about the flows it serves.

---

## L2 — User application-layer rate limiting

Expressed as a **built-in stateful internal middleware** called `rate_limit`. Opt-in per rule. Purely for application-layer concerns (per-user API limits, per-IP bandwidth limits, per-endpoint load shedding).

### Algorithm: Token Bucket only

Only algorithm supported. Single, understood, cheap.

```rust
pub struct RateLimitMiddleware {
    buckets: DashMap<Key, TokenBucket>,
    config:  RateLimitConfig,
}

pub struct RateLimitConfig {
    pub key:       KeyDerivation,
    pub rate:      u32,              // tokens refilled per window
    pub burst:     u32,              // bucket capacity
    pub window:    Duration,         // 1s to 60s inclusive
    pub on_limit:  OnLimit,
}
```

### Time window: 1 to 60 seconds

**`window` must be in `[1s, 60s]`**. A config value outside this range is a compile error:

```
Error: rate_limit window exceeds 60s (got 120s) in rule "foo".
Hint: The proxy is not the right place for long-window rate limiting.
Rate limits with windows > 60s should be implemented in the upstream
application, where the limit can be backed by persistent state and
survive proxy restarts.
```

The upper bound reflects a philosophical stance: `vane` is an in-memory single-node rate limiter; it does not claim to replace distributed rate limiting with multi-node consistency. Windows beyond a minute cross the line into "needs persistent, shared, durable state" territory — a database job, not a proxy job.

Different rules may use different window values freely within `[1s, 60s]`. Mixing is supported; different windows simply imply different internal cleanup cadences for their token-bucket state.

### Key derivation

```rust
pub enum KeyDerivation {
    RemoteIp,
    Header(String),                      // e.g., "x-api-key"
    Cookie(String),
    Query(String),
    Composite(Vec<KeyDerivation>),       // e.g., (RemoteIp, Header("x-api-key"))
    Global,                              // one bucket for all — combined with rule predicates, this means "global cap on matching paths"
}
```

### "Per X per path" and "Global per path"

These patterns fall out of combining rule predicates (which decide _which requests_ the rate limit applies to) with `KeyDerivation` (which decides _how those requests are bucketed_):

**Per-IP per-path**:

```json
{
	"match": [{ "http.uri.path": { "prefix": "/api/" } }],
	"use": "rate_limit",
	"args": { "key": "remote_ip", "rate": 5, "burst": 10, "window": "1s" }
}
```

**Global per-path**:

```json
{
	"match": [{ "http.uri.path": { "prefix": "/api/" } }],
	"use": "rate_limit",
	"args": { "key": "global", "rate": 300, "burst": 500, "window": "1s" }
}
```

Multiple `rate_limit` nodes on the same path stack — e.g., a per-IP limit at 10/s combined with a global limit at 300/s creates a two-tier shield.

### On-limit response

```rust
pub enum OnLimit {
    Reject {
        status:  u16,             // default 429
        headers: HeaderMap,       // may include "retry-after"
        body:    Bytes,           // body of the rejection response
    },
}
```

Only `Reject` is supported in MVP. `Delay` (briefly queue hoping for a token) is deliberately not implemented — queueing under sustained overload creates worse failure modes than fast rejection.

### State storage

**Local RAM (`DashMap`) only.** No Redis, no shared storage, no distributed consistency. Single-node semantics.

Multi-daemon deployments behave as N independent limiters — the effective total rate equals `N × configured rate`. This is the correct behavior for a per-node proxy that happens to be replicated; distributed rate limiting is an application-layer concern, not a proxy concern (same philosophy as the 60s window ceiling).

---

## Positional summary

|                 | L1 Security                         | L2 User                                      |
| --------------- | ----------------------------------- | -------------------------------------------- |
| Position        | Listener / pre-handshake / parse    | FlowGraph middleware node                    |
| Can be disabled | No — every limit always present     | Yes — opt-in per rule                        |
| Goal            | Keep the `vaned` process alive      | Keep upstream applications / logic protected |
| Trigger outcome | close / RST (no HTTP response room) | 429 / custom response                        |
| State scope     | per-IP / per-conn / daemon-global   | arbitrary dimension via `KeyDerivation`      |
| Algorithm       | fixed (counter / leaky)             | Token Bucket (single algorithm)              |
| Configuration   | env vars via dotenvy                | per-rule in `rules/*.json`                   |
| Log level       | `warn` + dedup                      | normal flow log                              |
| Window          | fixed (per limit)                   | 1s to 60s, compile-checked                   |
| Distributed     | n/a                                 | no (local RAM only, by design)               |
