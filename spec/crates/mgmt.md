# vane-mgmt

Source: [`crates/mgmt/`](../../crates/mgmt/).

The management protocol — one wire format, two transports, shared by daemon and CLI / TUI. CLI / TUI are clients; neither is a privileged in-process consumer.

## Owns

- Wire format: `Request` / `Response` / `Stream` frame shapes, JSON-over-line and JSON-over-HTTP serialisation. Source: `protocol.rs`.
- Verb schemas. Source: `verb.rs`.
- Server: mounts onto a Unix socket or HTTP-over-TCP. Source: `server.rs`, `http_server.rs`.
- Client: typed client against the same verb set. Source: `client.rs`, `http_client.rs`.

## Crate dependencies

`vane-core` + `tokio`, `hyper` (for the HTTP transport), `serde_json`. NDJSON over chunked is the chosen streaming mechanism — `tokio-tungstenite` is deliberately not a dependency anywhere in the workspace; reviewers should flag accidental imports.

## Transports

- **Unix socket** (default-on). Path: `$XDG_RUNTIME_DIR/vaned.sock`, else `/var/run/vaned.sock`. No auth — filesystem permissions are the boundary. Recommended mode `0660` with group `vaneadm`.
- **HTTP-over-TCP** (default-on, port 3333). Plaintext H1.1 only. Bind defaults to loopback (`127.0.0.1` + `[::1]`); `VANE_MGMT_HTTP_PUBLIC` opts into wildcard bind when the operator wants the admin port reachable as-is. Bearer-token auth on `Authorization: Bearer <token>`. TLS / H2 / mTLS / ACME for the management endpoint are not vane-mgmt's concern — operators who want them write a vane reverse-proxy rule fronting the loopback management endpoint (see § _Auth model_ below).

Both transports carry identical request/response shapes.

## Wire format

### Unix socket

Line-delimited JSON. One request per line, one response per line. Streaming verbs emit multiple response lines before a terminator frame.

### HTTP-over-TCP

JSON body over POST for request/response verbs. `application/x-ndjson` chunked response for streaming verbs.

### Streaming verb lifecycle

Streaming verbs (`tail_flow`, `tail_log`):

- **Start** — client sends a POST (HTTP) or a request line (Unix). The daemon emits `{"request_id": ..., "stream": {"seq": N, "data": ...}}` frames.
- **Cancel** — client closes the TCP (or Unix) connection. No control-frame vocabulary; closing the transport is the cancellation signal. The daemon sees the close, drops its subscriber, reclaims resources.
- **Parameter changes** (filter, level, conn-id scope) — not supported mid-stream. Client cancels and issues a fresh request with new args. Reconnection is cheap; the stream has no persistent server-side state beyond the subscriber binding.
- **Back-pressure and overflow** — implemented via `tokio::sync::broadcast` with a bounded channel per stream kind. Each subscriber holds its own `Receiver`; when it cannot keep up, `recv().await` returns `RecvError::Lagged(n)`. The streamer converts to `{"stream": {"dropped": N, "reason": "backpressure"}}` and continues. Other subscribers are unaffected. Channel capacity defaults: flow log 4096 events, structured log 1024 lines.

This avoids a second protocol layer (WebSocket control frames, SSE event types) and keeps `vane` CLI clients trivial.

### Request / response shapes

```json
{ "verb": "compile_dry_run", "args": { "config_dir": "/etc/vaned" }, "request_id": "xyz-123" }

{ "request_id": "xyz-123", "result": { ... }, "error": null }

{ "request_id": "xyz-123", "stream": { "seq": 42, "data": { ... } } }
{ "request_id": "xyz-123", "stream": { "end": true, "reason": "done" | "error" } }
```

## Verbs

Verb names use snake_case on the wire. Read verbs are uniformly prefixed `get_`; streaming verbs `tail_`; one-shot actions stand alone (`reload`, `shutdown`, `compile_dry_run`). The CLI mirrors this — `vane get config`, `vane tail flow`, `vane reload` — see [`cli.md` § _Subcommand layout_](cli.md#subcommand-layout) for the full mapping.

### Configuration

- `compile_dry_run` — take a config directory path, return the compiled FlowGraph plus diagnostics. Pure; no side effects.
- `reload` — trigger re-read / re-compile / swap.
- `get_config` — return the current `MergedConfig` and compiled `FlowGraph` hash.

### Observability

- `get_connections` — snapshot of live connections (remote, local, transport, age, bytes, current node).
- `tail_flow` — stream flow-path events: predicate evaluation, terminator invocation.
- `tail_log` — stream the structured log.
- `get_metrics` — counter / gauge snapshot. Backend is the [`metrics`](https://crates.io/crates/metrics) crate (facade) with `metrics-exporter-prometheus`. Args: `format: "prometheus" | "json"`, default `"prometheus"` (text exposition format suitable for scraping). All counters / gauges go through `metrics::counter!` / `gauge!` / `histogram!` macros — no bespoke facade.

  Exposing metrics through the management verb means Prometheus scrapers must authenticate (bearer token on HTTP transport, file-permission boundary on Unix). The standard scrape pattern (`GET /metrics` unauthenticated on a dedicated port) is intentionally not the default — vane treats metrics as privileged information. Operators who want a scrape-friendly endpoint bridge via `curl -H "Authorization: Bearer $TOKEN" ...` piped to a sidecar.

```rust
// TODO(metrics-public-bind): a dedicated VANE_METRICS_HTTP_BIND for
// scrape-first workflows is reserved if it becomes common. Until then,
// the bridge-via-curl pattern above is the route.
```

### Runtime

- `stats` — daemon summary: uptime, active connections, FlowGraph version hash, WASM pool status.
- `shutdown` — graceful shutdown (drain, wait, exit).

### State

- `get_pools` — per stateful WASM module: pool size, in-use count, total allocations, failures.
- `get_upstreams` — pooled HTTP upstream connections (hyper-util client) and QUIC associations.
- `get_certs` — managed + static certs with status, SAN list, expiry, last-attempt time, last error. Status is `valid | renewing | failed | limited`. Response shape and field semantics in [`engine-acme.md` § _mgmt verbs_](engine-acme.md#mgmt-verbs).

### Certificates

- `force_renew` — trigger immediate renewal for one managed cert. Args `{ "sni": string }`; returns `{ "queued": bool, "current_status": string }`. Bypasses the periodic renewal timer and the ARI-suggested window. Useful for key-compromise rotation.

### Pools

- `pool_drain` — drop one cached upstream entry by fingerprint. Args `{ "fingerprint": string }`. Useful for forced rotation after cert refresh.

## Auth model

### Unix socket

No auth. `chmod 0660 /var/run/vaned.sock`, group `vaneadm`. Users in that group have full control.

### HTTP-over-TCP

- **Bearer token** in `Authorization: Bearer <token>`. Plaintext via `VANE_MGMT_HTTP_TOKEN` env var. Token comparison is constant-time so a malformed `Authorization` header cannot be timing-fingerprinted.
- **No TLS.** Plaintext HTTP/1.1 only. Operators wanting TLS terminate it via a vane reverse-proxy rule.

The `(public, token)` pairing decides whether the daemon starts:

| `VANE_MGMT_HTTP_PUBLIC` | `VANE_MGMT_HTTP_TOKEN` | Boot behavior                                                                                                               |
| ----------------------- | ---------------------- | --------------------------------------------------------------------------------------------------------------------------- |
| unset / `0` / `false`   | unset                  | Bind loopback, **warn** ("management HTTP is unauthenticated; same-host users on this machine can issue management calls"). |
| unset / `0` / `false`   | set                    | Bind loopback, enforce token.                                                                                               |
| set (`1` / `true`)      | unset                  | **Refuse to start** — wildcard bind without a token would expose the management API plaintext on the public network.        |
| set (`1` / `true`)      | set                    | Bind wildcard, enforce token.                                                                                               |

`VANE_BIND_IPV4` / `VANE_BIND_IPV6` decide which families participate. With both 1, loopback bind expands to `127.0.0.1:3333` + `[::1]:3333`; public bind to `0.0.0.0:3333` + `[::]:3333`.

There is no per-verb RBAC. Management is root-level: you have access or you don't. Finer-grained auth is not in scope.

### TLS via vane-on-vane

Plaintext-only is intentional. Operators wanting HTTPS / H2 / mTLS / ACME on the admin endpoint write a vane rule that fronts the loopback HTTP transport, reusing the engine's full TLS surface (certs, ALPN, ticketer, listener kind derivation, hot reload). Two-port deployment, no port conflict:

```json
{
	"rule": "vane-admin",
	"listen": [":443"],
	"match": [{ "tls.sni": { "equals": "admin.example.com" } }],
	"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:3333" },
	"tls": { "cert_file": "/etc/vaned/admin.crt", "key_file": "/etc/vaned/admin.key" }
}
```

Operator hits `https://admin.example.com/` from anywhere; the rule terminates TLS, vane proxies to its own loopback admin port. Wildcard bind on the management transport (`VANE_MGMT_HTTP_PUBLIC=1`) and reverse-proxy fronting are independent choices: the former is "I'm on a trusted network and want the admin port reachable as-is"; the latter is "I want HTTPS termination plus everything else vane already does for proxied traffic."

## Idempotency

- `reload` is idempotent when the config has not changed (same version hash returned).
- `compile_dry_run` is pure.
- `get_*` are read-only snapshots.
- `shutdown` is not idempotent (runs once per daemon lifetime).

## Errors

Errors serialise from the `Error` shape in [`core.md` § _Error type_](core.md#error-type):

```json
{
	"error": {
		"kind": "Compile",
		"message": "duplicate rule name 'web-api' in rules/30-web.json and rules/40-web.json",
		"source": null
	}
}
```

Kinds surfaced on management: `Compile`, `Timeout`, `Internal`. Runtime kinds (`Io`, `Protocol`, `Upstream`) surface via the flow log and structured log, not as management errors.

## Tests

`crates/mgmt/tests/`:

- `end_to_end.rs` — Unix transport: real socket file, real `UnixListener`, real client connections. Exercises bind, perms, dispatch, cancel-and-cleanup, concurrent client connections.
- `http_transport.rs` — HTTP-over-TCP: spins `spawn_http_server` against a stub `Handler` on a fresh ephemeral port; drives requests through `HttpMgmtClient` (typed) or raw TCP write (for malformed requests the typed client cannot construct).
