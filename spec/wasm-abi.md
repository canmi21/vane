# WASM ABI

Authoritative wire contract between `vaned` and external WASM plugins. This file is the single source of truth for the WIT shape, host-function surface, error model, and lifecycle obligations that plugin authors depend on. Adding a field, renaming a record, or changing a function signature is an ABI change and follows the versioning rules below.

Runtime behavior, instance pool model, observability, dedup, and policy concerns live in [`architecture/11-wasm.md`](architecture/11-wasm.md). This file specifies _only_ the contract.

## Versioning

- Package: `vane:plugin@<major>.<minor>.<patch>`. Stage 3 ships `vane:plugin@0.1.0`.
- Plugins declare the package version they target via `metadata.abi-version`. The host rejects loading any component whose `abi-version` major differs from the host's.
- **Additive minor bump** (host accepts plugins built against the previous minor): adding optional record fields, adding `context-value` variants, adding host functions, widening accepted `on-error-hint` values.
- **Major bump** (recompilation required): renaming or removing fields, narrowing variants, changing function signatures, tightening trap conditions.

## World

A plugin's world imports the host interface and exports `registry` plus zero-or-more kind-specific handler interfaces — exactly the kinds the plugin implements.

```wit
// jwt-validator.wit (plugin author writes this)
package my-plugins:jwt@1.0.0;

world jwt-validator {
    import vane:host/host@0.1.0;
    export vane:plugin/registry@0.1.0;
    export vane:plugin/handler-l7-request@0.1.0;
}
```

The host introspects which `vane:plugin/handler-*` interfaces the component exports and cross-checks against `metadata.exports`.

## Registry

Single function called once per component load. Returns static metadata describing every middleware exported by the component.

```wit
package vane:plugin@0.1.0;

interface registry {
    use types.{metadata};
    get-metadata: func() -> metadata;
}

interface types {
    enum middleware-kind {
        l4-peek,
        l4-bytes,
        l7-request,
        l7-response,
    }

    record middleware-export {
        // Export name within the component (e.g. "jwt-validator").
        // Appears in rule config as "<module>:<name>".
        name: string,

        kind: middleware-kind,

        // Pool model: stateless instances are reused via PoolingAllocator;
        // stateful instances are pre-allocated per call site.
        stateless: bool,

        // Drives LazyBuffer activation at compile time. For l7-response
        // middleware, this refers to response body.
        needs-body: bool,

        // Capability declaration. The host packs ONLY paths declared here
        // into `context` on each call; reading other paths is impossible.
        // Path grammar: see § Context exposure.
        inspects: list<string>,

        // Reserved for future streaming-body extension. Must be false in
        // 0.1.0; the host rejects components whose any export sets this true.
        needs-streaming-body: bool,
    }

    record metadata {
        // Logical plugin name (informational; metric / log label).
        name: string,
        // Plugin semver (informational).
        version: string,
        // ABI version this component targets. Must equal "0.1.0" or the
        // host rejects the component.
        abi-version: string,
        exports: list<middleware-export>,
    }
}
```

The host rejects load if:

- `abi-version` major differs from the host's.
- Any `middleware-export.kind = K` lacks the corresponding `handler-K` interface export.
- Any `middleware-export.needs-streaming-body = true`.
- Two `middleware-export` entries share the same `name`.

## Per-kind handlers

One interface per `middleware-kind`. A plugin exports only the interfaces matching the kinds it implements. Within an interface, `handle` takes a `name` parameter selecting which export within that kind handles the call — this lets a single component export multiple middlewares of the same kind (e.g. two distinct l7-request validators).

### `handler-l4-peek`

```wit
package vane:plugin@0.1.0;

interface handler-l4-peek {
    use types.{plugin-error, context-entry};

    record l4-peek-input {
        // Bytes peeked from the connection; up to the host's peek-prefix
        // limit (default 8 KiB).
        peek: list<u8>,

        // Field paths declared in `inspects`, packed by host.
        context: list<context-entry>,
    }

    variant l4-peek-decision {
        continue,
        close,
    }

    handle: func(name: string, input: l4-peek-input)
        -> result<l4-peek-decision, plugin-error>;
}
```

### `handler-l4-bytes`

```wit
interface handler-l4-bytes {
    use types.{plugin-error, bytes-view, context-entry};

    record l4-bytes-input {
        bytes: bytes-view,
        context: list<context-entry>,
    }

    variant l4-bytes-decision {
        continue,
        tunnel,
        close,
    }

    handle: func(name: string, input: l4-bytes-input)
        -> result<l4-bytes-decision, plugin-error>;
}
```

### `handler-l7-request`

```wit
interface handler-l7-request {
    use types.{plugin-error, header, bytes-view, context-entry};

    record l7-request-input {
        // Upper-case ASCII (e.g. "GET", "POST").
        method: string,
        // Request-target as on the wire (origin-form for proxied requests).
        uri: string,
        // Names are ASCII-lowercase; values are UTF-8 (see § Headers).
        headers: list<header>,
        // Present iff the export's `needs-body` is true.
        body: option<bytes-view>,
        context: list<context-entry>,
    }

    record synth-response {
        status: u16,                 // [100, 599]
        headers: list<header>,       // host normalizes names on emit
        body: list<u8>,
    }

    variant l7-request-decision {
        continue,
        short(synth-response),
        close,
    }

    handle: func(name: string, input: l7-request-input)
        -> result<l7-request-decision, plugin-error>;
}
```

`l7-request-decision` deliberately lacks any "route to node X" variant. Plugins decide; the FlowGraph routes. This keeps plugin reasoning local to its own input.

### `handler-l7-response`

```wit
interface handler-l7-response {
    use types.{plugin-error, header, bytes-view, context-entry};

    record l7-response-input {
        status: u16,
        headers: list<header>,
        // Present iff the export's `needs-body` is true.
        body: option<bytes-view>,
        context: list<context-entry>,
    }

    record modified-response {
        // none = leave unchanged; some = full replacement.
        status: option<u16>,
        headers: option<list<header>>,
        body: option<list<u8>>,
    }

    variant l7-response-decision {
        continue,
        modify(modified-response),
        abort,
    }

    handle: func(name: string, input: l7-response-input)
        -> result<l7-response-decision, plugin-error>;
}
```

`abort` causes the response delivery to fail with the connection closed; the rule's `on_error` does not apply (response middleware runs after the response was committed in the abstract sense, so retry-or-recover routing is ill-defined).

## Args delivery

Per-rule plugin args (the `args` JSON in rule config) are delivered **once per instance lifetime**, not on every call. The plugin retrieves them via the host import:

```wit
get-args: func() -> string;
```

- The returned string is always JSON; minimum value is `"{}"`.
- Values are stable for the instance's entire lifetime: stateless pool instances see the args of whichever rule rented them (since stateless dedup is `(module_id, export_name, args_canonical_json)`, all rentals through one `MiddlewareId` share one args value); stateful pool instances see the args of the call site they belong to.
- Re-reading `get-args` returns the same value. Plugins typically cache + parse it once during construction.

Rationale: args are configuration, not request data. Per-call repetition wastes serialization on every invocation.

## Body model

```wit
record bytes-view {
    data: list<u8>,
    truncated: bool,
}
```

- `data` carries up to the kind's body limit. Defaults: 1 MiB request, 1 MiB response, 64 KiB l4-bytes. Per-plugin override via plugin config.
- `truncated: true` means the actual body exceeded the limit; `data` holds the prefix.
- The plugin chooses fail-closed (return `plugin-error`) or proceed-with-prefix based on `truncated`.
- `body: option<bytes-view>` is `none` whenever `metadata.exports[].needs-body = false` — plugins that did not declare body need do not see body data.

Streaming bodies are not supported in 0.1.0. The `needs-streaming-body` reserved field on `middleware-export` is the forward-compatibility hook; setting it true today causes load rejection.

## Headers

```wit
record header {
    // Host guarantees ASCII-lowercase.
    name: string,
    // UTF-8. Non-UTF-8 byte values are escaped as `\x{HH}`.
    value: string,
}
```

- Inbound headers (in `*-input`) have names lowercased by the host.
- Multiple headers with the same name preserve their wire order in the list.
- Outbound headers (in `synth-response`, `modified-response`) need not be lowercased; the host normalizes before emission.
- Header values containing CR, LF, or null bytes in plugin output trap (see § Trap conditions).

## Context exposure

`context: list<context-entry>` carries connection and request fields the middleware declared in `inspects`.

```wit
record context-entry {
    path: string,
    value: context-value,
}

variant context-value {
    text(string),
    bytes(list<u8>),
    int64(s64),
    uint64(u64),
    boolean(bool),
    list-text(list<string>),
}
```

**Capability semantics**: the host packs **only** paths declared in `inspects`. Reading any other field is impossible — the data is not delivered. Path declarations are validated at plugin load: unknown paths cause load rejection. This makes `inspects` a real capability declaration and lets FlowGraph compile-time analysis (LazyBuffer activation, predicate sharing, mTLS gating) be sound.

### Path grammar

Connection-level paths:

| Path                                    | `context-value` | Notes                                                              |
| --------------------------------------- | --------------- | ------------------------------------------------------------------ |
| `conn.peer_ip`                          | `text`          | Textual representation (e.g. `"192.0.2.5"`).                       |
| `conn.peer_port`                        | `uint64`        | 0–65535.                                                           |
| `conn.local_ip`                         | `text`          |                                                                    |
| `conn.local_port`                       | `uint64`        |                                                                    |
| `conn.transport`                        | `text`          | `"tcp"` \| `"udp"` \| `"quic"`.                                    |
| `conn.alpn`                             | `text`          | Empty string if no ALPN.                                           |
| `conn.id`                               | `text`          | `ConnId` hex.                                                      |
| `conn.accept_unix_ms`                   | `uint64`        |                                                                    |
| `conn.tls.version`                      | `text`          | `"1.2"` \| `"1.3"` \| `""` if not TLS.                             |
| `conn.tls.sni`                          | `text`          | ASCII-lowercase. Empty if no SNI.                                  |
| `conn.tls.peer_cert`                    | `bytes`         | DER-encoded leaf cert. Empty if no client cert.                    |
| `conn.tls.peer_cert.present`            | `boolean`       | `true` iff a verified peer cert is attached.                       |
| `conn.tls.peer_cert.subject_cn`         | `text`          | Empty when `present == false`.                                     |
| `conn.tls.peer_cert.san_dns`            | `list-text`     | DNS-type SAN list. Empty when `present == false`.                  |
| `conn.tls.peer_cert.fingerprint_sha256` | `text`          | Hex (lowercase). SHA-256 of the full leaf DER.                     |
| `conn.tls.peer_cert.spki_sha256`        | `text`          | Hex (lowercase). SHA-256 of SubjectPublicKeyInfo. Rotation-stable. |
| `conn.tls.peer_cert.issuer_cn`          | `text`          |                                                                    |
| `conn.tls.peer_cert.serial`             | `text`          | Hex (lowercase). Big-endian, no leading-zero stripping.            |

Request / response paths are also declarable; declare them only when the middleware needs the value via the `context` channel (e.g. for predicate-style sharing) rather than reading the corresponding field on `*-input`. The path table mirrors `architecture/18-predicate-schema.md`.

The `inspects` mechanism replaces ad-hoc per-call ConnContext arguments and is the only way a plugin learns connection metadata.

## plugin-error

```wit
record plugin-error {
    // Short stable identifier (e.g. "policy.denied", "input.malformed").
    code: string,
    // Operator-facing description.
    message: string,
    // Routing hint for the host's error channel.
    on-error-hint: option<string>,
}
```

`on-error-hint` interpretation:

| Value           | Meaning                                                                                                               |
| --------------- | --------------------------------------------------------------------------------------------------------------------- |
| `none`          | Default. Use the rule's `on_error` config (see `architecture/04-middleware.md` § _Two error channels_).               |
| `"force-close"` | Ignore `on_error`; close connection (L4) or send 500 + close (L7). Reserved for unrecoverable plugin-internal errors. |
| `"internal"`    | Treat as internal anomaly: log + emit metric + apply `on_error` tombstone. Routine errors should not use this.        |

Other hint values trap (treated as malformed plugin output).

`plugin-error` is distinct from a trap. Returning `plugin-error` is an in-band, plugin-designed outcome and does not surface as a wasmtime trap. See `architecture/11-wasm.md` § _Trap and error handling_ for the dual-channel semantics.

## Host functions

A single import: `vane:host/host@0.1.0`. All functions are sync from the plugin's perspective; the host's wasmtime async-bridge handles concurrency.

```wit
package vane:plugin@0.1.0;

interface host {
    use types.{plugin-error};

    // -- args -----------------------------------------------------------

    get-args: func() -> string;

    // -- logging --------------------------------------------------------

    enum log-level { trace, debug, info, warn, error }

    record log-field {
        key: string,
        // Stringified value: numeric → decimal, bool → "true"/"false".
        // UTF-8 required (see § Trap conditions).
        value: string,
    }

    log: func(level: log-level, message: string, fields: list<log-field>);

    // -- time / random --------------------------------------------------

    now-unix-ms: func() -> u64;
    random:      func(buf-len: u32) -> list<u8>;

    // -- metrics --------------------------------------------------------

    record metric-label {
        key: string,
        value: string,
    }

    // The host enforces a per-plugin cardinality cap (default 1000 series).
    // Emissions exceeding the cap are dropped and a single warn-level
    // log is emitted per cap event per plugin.
    metric-counter: func(name: string, delta: u64, labels: list<metric-label>);
    metric-gauge:   func(name: string, value: s64, labels: list<metric-label>);

    // -- http-fetch -----------------------------------------------------

    record http-fetch-request {
        // Upper-case ASCII.
        method: string,
        // Absolute URI per RFC 3986.
        url: string,
        headers: list<tuple<string, string>>,
        body: list<u8>,
        // Per-call timeout. Falls back to plugin config default,
        // then daemon default (30 s).
        timeout-ms: option<u32>,
        // 0 disables redirects. Falls back to plugin config default
        // (default 5).
        follow-redirects: option<u32>,
        // Per-call insecure flag. Honored only when plugin config has
        // `allow-insecure: true`; otherwise ignored and TLS is verified.
        verify-tls: option<bool>,
    }

    record http-fetch-response {
        status: u16,
        headers: list<tuple<string, string>>,
        // Truncated to plugin config max-body-size (default 1 MiB).
        body: list<u8>,
    }

    variant net-error {
        dns-failure(string),
        connection-refused,
        timeout,
        tls-error(string),
        pool-exhausted,
        body-too-large,
        not-allowed(string),     // outside `allowed_hosts`
        insecure-rejected,       // verify-tls=false but allow-insecure=false
        internal(string),
    }

    http-fetch: func(req: http-fetch-request) -> result<http-fetch-response, net-error>;
}
```

`http-fetch` shares the daemon's `TcpPool` (same fingerprint, same observability) via the `HttpFetchBackend` trait declared in `vane-core`. Policy detail (allowed_hosts default, default ClientConfig, mTLS overrides) lives in `architecture/11-wasm.md`.

## Module identity and reload

`module_id` is the **canonical absolute filesystem path** of the `.wasm` file (e.g. `/etc/vaned/wasm/jwt-validator.wasm`).

On hot reload of a path:

1. Compute content hash; deserialize or compile per `architecture/11-wasm.md` § _Boot_.
2. Invoke `registry.get-metadata()` on the new component.
3. Compare new metadata to cached for that `module_id`:
   - If `(kind, stateless, needs-body, inspects)` matches per export _and_ the export-name set is identical — **module-only swap**. The FlowGraph is not recompiled; the `MiddlewareInst::Wasm` continues to refer to `module_id`, and instances rented after the swap construct against the new component.
   - Otherwise — **metadata-changed reload**. Triggers a full FlowGraph recompile.
4. `metadata.name` and `metadata.version` changes alone do not affect routing; they only annotate metric and log labels.

Renaming or moving a `.wasm` file is treated as deletion + addition: the old `module_id` drops with its FlowGraph generation; the new one compiles into the next graph generation.

## Cancellation

The ABI does not propagate cancellation signals. Plugin invocations run to completion or hit the per-call epoch deadline (default 10 ms; configurable per plugin). Client disconnect mid-invocation is not signaled to the plugin; the plugin's eventual return is discarded by the host.

Rationale: the 10 ms ceiling makes proactive cancellation a marginal optimization. A future minor ABI version may add `host.is-cancelled() -> bool` if profiling justifies it.

## Epoch tick frequency

The host increments the wasmtime engine's epoch counter every **1 ms**. Combined with the default 10 ms per-call deadline, plugin invocations are preempted within `10 ms ± 1 ms`. Tick frequency is fixed (not configurable per plugin) so host-side overhead stays constant regardless of plugin count.

## Forward-compatibility hooks

Reserved fields and values, intentionally unused in 0.1.0, that future minor versions may activate without a major bump:

- `middleware-export.needs-streaming-body` — when true (rejected today), enables resource-handle body streaming.
- `plugin-error.on-error-hint` — additional string values may be added.
- Keys starting with `vane.` in `log-field` and `metric-label` are reserved for host-injected fields; plugins must not emit them.

## Trap conditions

Conditions that trap (the host's `bindgen!` shim returns `Err` to the engine, treated as internal anomaly per `architecture/11-wasm.md` § _Trap and error handling_):

- Returning a `plugin-error.on-error-hint` value not in `{none, "force-close", "internal"}`.
- Returning a `synth-response`, `modified-response`, or `header` whose `name` or `value` contains CR, LF, or null bytes.
- Returning `synth-response.status` or `modified-response.status` outside `[100, 599]`.
- Calling `host.log` with non-UTF-8 bytes in `message` or any `log-field.value`.
- Calling `host.http-fetch` with a `url` that fails RFC 3986 absolute-URI validation.
- Calling `host.metric-counter` or `host.metric-gauge` with a `name` outside `[a-zA-Z_][a-zA-Z0-9_]*`.
- Returning a WIT-decoded value the wasmtime host bindings cannot deserialize. Listed for completeness; the bindings already trap these.

`plugin-error` returned via `result.err` is **not** a trap — it flows through the regular middleware error channel.
