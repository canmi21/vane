# Presets

Presets are **opinionated compile-stage expansions** that turn high-level intent into raw rule bundles. They let common deployments ship with sensible defaults while keeping the raw rule layer transparent.

## Two-tier rule system

```
User config (mix of raw rules + presets)
  ↓ preset expansion  ── new pipeline stage
Canonical raw rules
  ↓ merge / analyze / lower / validate  ── existing core pipeline
Arc<SymbolicFlowGraph>
  ↓ link              ── engine-only; resolves names → MiddlewareInst / FetchInst
Arc<FlowGraph>
```

Preset expansion produces **`RawRule`s** — the same shape hand-written raw rules take. No preset ever produces `MiddlewareInst` or `FetchInst` directly; those trait-object types are engine's responsibility and only come into existence at link time. See `02-flow.md` § _Compile and link_ for the full pipeline.

- **Raw rule layer** — what the user writes is exactly what the FlowGraph runs. Zero implicit middleware injection, zero hidden defaults. A raw rule without a `rate_limit` truly has no rate limit. A raw rule without `forward_client_ip` truly does not add `X-Forwarded-For`.
- **Preset layer** — "usually-on" policies (sensible rate limit, client IP forwarding, WebSocket handling, timeouts) live here. A preset expands to one or more raw rules; the expansion is fully visible via `vane compile --dry-run`.

Users choose their posture:

- **Hand-write raw rules** — maximum control, zero surprises, no policy unless explicitly added.
- **Use presets** — convenience with policy opinions baked in; inspect what the preset does with `--dry-run`.
- **Mix** — presets for the common cases, raw rules for the edges.

## Design principle: transparent expansion

The compiled FlowGraph loaded into memory has **no hidden behavior**. The engine does not add middleware behind the user's back. Presets produce their effects only by emitting explicit raw rules; those raw rules run through the same compile pipeline as hand-written ones.

Consequence: `vane compile --dry-run` output is the authoritative description of what will execute. Reading it tells you everything the daemon will do, with no "and also these invisible things" footnote.

## Preset expansion stage

Implemented as a pure function:

```rust
fn expand(preset: PresetInvocation) -> Vec<RawRule>;
```

Each preset has its own expansion rule. Expansion runs before merge; expanded rules participate in merge and compile like any other raw rule.

### `RawRule` shape

The intermediate form a rule takes between parse/expand and compile. Middleware and fetch are referenced by **string name** at this stage — the compiler's `lower` pass resolves those strings into `MiddlewareId` / `FetchId` using the middleware registry (see `04-middleware.md`).

```rust
pub struct RawRule {
    pub name:             String,                         // unique across the merged rule set
    pub listen:           Vec<ListenSpec>,                // ":443" / "0.0.0.0:80" / "udp:443" / "tcp:[::]:443"
    pub match_predicate:  Option<Predicate>,              // config-form predicate (see 18-predicate-schema.md)
    pub middleware_chain: Vec<MiddlewareRef>,             // middleware nodes, in declared order
    pub terminate:        TerminateSpec,                  // single JSON block that names both the Fetch and (implicitly) the Terminator
    pub source:           SourceInfo,                     // which file + line produced this rule
}

#[derive(serde::Deserialize)]
pub struct MiddlewareRef {
    #[serde(rename = "use")]
    pub name:     String,                 // registry key — e.g., "rate_limit" or "auth:jwt_validator"
    #[serde(default)]
    pub args:     serde_json::Value,      // per-instance args, opaque to the registry
    #[serde(default)]
    pub on_error: Option<OnErrorSpec>,    // how Err(_) returns are routed; None → fail-safe tombstone
}
// JSON shape (flat, Form 1):
//   { "use": "rate_limit", "args": { "rate": 100 }, "on_error": "close" }
// `args` defaults to null (Value::Null) when omitted; on_error defaults to None
// (which means the fail-safe tombstone per 04-middleware.md).

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum OnErrorSpec {
    Close,                                // L4 RST / L7 close
    Response(SynthResponse),
    // post-MVP: Rule(String)  — jump to another rule's entry
}
// Custom Deserialize dispatches on shape:
//   "close"                                   → OnErrorSpec::Close
//   { "response": { "status": N, ... } }      → OnErrorSpec::Response(SynthResponse)
// See 04-middleware.md § _Config form_.

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct SynthResponse {
    pub status:  u16,
    #[serde(default)]
    pub headers: Option<std::collections::BTreeMap<String, String>>,
    #[serde(default)]
    pub body:    Option<String>,
}
// SynthResponse's field types are deliberately serde-friendly — `BTreeMap<String,
// String>` for headers (not `http::HeaderMap`, which needs custom
// deserialization) and `String` for body (not `Bytes`, which cannot
// round-trip through plain JSON). This is the config-time wire shape;
// the fallback-response builder downstream converts into `http::HeaderMap`
// and `Bytes` when materializing the synth response. Invalid header
// names / values surface at that conversion step, not at parse time.

// `TerminateSpec` mirrors the user-facing JSON exactly:
//   "terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8080", "timeouts": {...} }
// `type` maps to a FetchKind (see 05-terminator.md § _Variant ergonomics in config_);
// every other key goes into `args` verbatim. The Terminator variant
// (WriteHttpResponse vs ByteTunnel) is derived from the FetchKind at lower
// time and is not carried in the source JSON — it is redundant with the kind.
pub struct TerminateSpec {
    pub kind: FetchKind,                  // parsed from the JSON "type" field
    pub args: serde_json::Value,          // all other keys (upstream, timeouts, headers, body, transport, ...)
}

pub struct SourceInfo {
    pub file: std::path::PathBuf,
    pub line: u32,                 // JSON pointer line, for error messages
}
```

The same `RawRule` shape is the output of both: (a) hand-written raw rules (parsed from `rules/*.json`) and (b) preset expansion. The merge stage treats them identically.

Expansion is **deterministic given the preset args plus current daemon config** (env vars, config.json). Two runs of `vane compile --dry-run` on the same inputs produce byte-identical output.

## Catalog

Four presets cover the MVP scope:

- `reverse_proxy` — derives `ListenerKind::Http`
- `port_forward` — derives `ListenerKind::Raw`
- `static_site` — derives `ListenerKind::Http`
- `redirect_https` — derives `ListenerKind::Http`

The `ListenerKind` is **not** a preset arg — it follows from which terminators the preset emits. See [`06-l4.md`](06-l4.md) § _Listener kind derivation_. A listener that mixes presets across categories (e.g., a `port_forward` rule and a `reverse_proxy` rule both listening on `:443`) derives `ListenerKind::Auto`; the listener then dispatches per-connection by [`06-l4.md`](06-l4.md) § _Dispatch decision table_.

---

### `reverse_proxy`

HTTP reverse proxy with sensible production defaults.

```json
{
	"preset": "reverse_proxy",
	"listen": [":443"],
	"args": {
		"upstream": "127.0.0.1:8080",
		"websocket": false,
		"rate_limit": { "rate": 100, "burst": 200, "window": "1s" },
		"forward_client_ip": true,
		"timeouts": { "connect": "5s", "total": "60s" }
	}
}
```

Defaults injected when args are omitted:

- **`websocket: false`** — WebSocket upgrade requests are rejected with 400. See [WebSocket](#websocket-handling) below.
- **`rate_limit`**: per-IP at 100/s with burst 200 (conservative against accidental DDoS, non-intrusive for legitimate traffic).
- **`forward_client_ip: true`** — adds `X-Forwarded-For` (append) and `X-Real-IP` (overwrite) to the upstream request.
- **`timeouts`**: 5s connect, 60s total.

Expands to (conceptually — **one** RawRule for the main path plus one small gate rule for the WS reject):

```
Rule <name>.ws   → match [upgrade == websocket] → HttpSynthesize 400    (only when websocket: false)
Rule <name>.main → match []
                   middleware_chain: [ rate_limit(...), forward_client_ip ]
                   → HttpProxy(upstream, timeouts)
```

Why a single `<name>.main` rule with a chain instead of three parallel rules: the middleware execution order inside `reverse_proxy` is a **pipeline** (`rate_limit` fires before `forward_client_ip` before `HttpProxy`), not a specificity competition. A chain on one rule preserves declaration order trivially. Three sibling rules would depend on inspection-level sort, which ranks higher-inspection rules first — putting `HttpProxy` (L7-header) _before_ `rate_limit` / `forward_client_ip` (both L4-only level), which is backwards. The chain form sidesteps the ordering question entirely.

The WS gate is a separate rule because it is a genuine predicate branch (upgrade requests take a different terminator), not a middleware pipeline step.

#### WebSocket handling

The `websocket` arg accepts:

| Value                    | Effect                                                                  |
| ------------------------ | ----------------------------------------------------------------------- |
| `false` (default)        | WS upgrade requests rejected with 400                                   |
| `true` or `"*"`          | All paths allow WS passthrough                                          |
| `["/ws", "/api/stream"]` | Only listed path prefixes allow WS; other WS requests rejected with 400 |

Expansion for `websocket: true`:

```
Rule <name>.ws   → match [upgrade == websocket]  → WebSocketUpgrade(upstream)
Rule <name>.main → match []                      → HttpProxy(upstream)
```

Expansion for `websocket: ["/ws"]`:

```
Rule <name>.ws-allow → match [upgrade == websocket AND path.prefix any_of ["/ws"]]
                     → WebSocketUpgrade(upstream)
Rule <name>.ws-deny  → match [upgrade == websocket]
                     → HttpSynthesize 400
Rule <name>.main     → match []
                     → HttpProxy(upstream)
```

`<name>.ws-allow` has more predicates → higher specificity → sorts before `<name>.ws-deny`, so matched paths take the WS route and unmatched WS requests fall through to rejection.

---

### `port_forward`

Raw L4 byte forward (TCP or UDP).

```json
{
	"preset": "port_forward",
	"listen": [":2222"],
	"args": {
		"upstream": "10.0.0.5:22",
		"transport": "tcp"
	}
}
```

Expansion:

```
Rule <name> → match [] → L4Forward(upstream, transport)
```

No middleware — L4 forwarding is intentionally spartan. No rate limiting, no client IP forwarding (there is no HTTP layer to set headers on).

---

### `static_site`

Static content or synthesized response.

```json
{
	"preset": "static_site",
	"listen": [":443"],
	"args": {
		"status": 200,
		"headers": { "content-type": "text/plain" },
		"body": "Hello, world!"
	}
}
```

Expansion:

```
Rule <name> → match [] → HttpSynthesize(status, headers, body)
```

Serving a file tree (rather than a single static response) is an MVP-adjacent feature, implemented as an extended variant once the underlying file-serving Fetch lands.

---

### `redirect_https`

HTTP-to-HTTPS redirect.

```json
{
	"preset": "redirect_https",
	"listen": [":80"]
}
```

Expansion:

```
Rule <name> → match [] → HttpSynthesize(
  status:  308,
  headers: { "location": "https://${host}${uri}" }
)
```

Status fixed at 308 (preserves request method on modern clients).

---

## Writing a preset (architectural outline)

Each preset is a Rust function `expand(args: Value) -> Vec<RawRule>`. Presets live in a daemon-internal registry; adding a new preset is a source change in the `vaned` crate.

User-defined presets (via WASM or config templates) are **not supported**. Presets encode policy opinions, and policy opinions should be reviewed at code-commit time rather than at configuration-load time. If users want custom expansions, they write raw rules directly.

---

## Relationship to `type` aliases in `terminate`

Not to be confused: the `type` string inside a rule's `terminate` block (e.g., `"type": "http_proxy"`) is a one-for-one syntactic alias for a `FetchInst` variant. That is not a preset; it's a raw-layer naming convenience.

Presets are distinguished by the **top-level `preset` field** on a rule object. They expand into multiple `FetchInst`-level rules, possibly with middleware, across the compile stage.
