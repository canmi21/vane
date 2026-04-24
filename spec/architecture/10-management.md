# Management Protocol

## Transports

One protocol, two transports:

- **Unix socket** (default-on). Path: `$XDG_RUNTIME_DIR/vaned.sock`, else `/var/run/vaned.sock`. No auth — filesystem permissions are the boundary. Recommended mode `0660` with group `vaneadm`.
- **HTTP-over-TCP** (opt-in). Bound address configurable, defaults to `127.0.0.1`. Bearer-token auth. TLS required when bound to non-loopback; refuse to start otherwise.

Both transports carry identical request/response shapes. TUI and CLI are clients; neither is a privileged in-process consumer.

## Wire format

### Unix socket

Line-delimited JSON. One request per line, one response per line. Streaming verbs emit multiple response lines before a terminator frame.

### HTTP-over-TCP

JSON body over POST for request/response verbs. `application/x-ndjson` chunked response for streaming verbs.

### Streaming verb lifecycle

Streaming verbs (`tail_flow_log`, `tail_log`) follow a minimal contract:

- **Start** — client sends a POST (HTTP) or a request line (Unix). The daemon begins emitting `{"request_id": ..., "stream": {"seq": N, "data": ...}}` frames.
- **Cancel** — client closes the TCP (or Unix) connection. No control-frame vocabulary; closing the transport is the cancellation signal. The daemon sees the close, drops its subscriber, and reclaims resources.
- **Parameter changes** (filter, level, conn-id scope) — **not supported mid-stream**. The client cancels and issues a fresh request with the new args. Reconnection is cheap; the stream has no persistent server-side state beyond the subscriber binding.
- **Back-pressure and overflow** — implemented via `tokio::sync::broadcast` with a bounded channel per stream kind (flow log, structured log). Each subscriber holds its own `broadcast::Receiver`; when it cannot keep up, `Receiver::recv().await` returns `RecvError::Lagged(n)`, which the streamer converts to a `{"stream": {"dropped": N, "reason": "backpressure"}}` frame and continues. Other subscribers are unaffected. Channel capacity defaults: flow log 4096 events, structured log 1024 lines.

This avoids a second protocol layer (WebSocket control frames, SSE event types) and keeps `vane` CLI clients trivial.

### Request

```json
{
	"verb": "compile_dry_run",
	"args": { "config_dir": "/etc/vaned" },
	"request_id": "xyz-123"
}
```

### Response

```json
{
  "request_id": "xyz-123",
  "result": { ... },
  "error": null
}
```

### Stream frame

```json
{ "request_id": "xyz-123", "stream": { "seq": 42, "data": { ... } } }
```

Terminal frame:

```json
{ "request_id": "xyz-123", "stream": { "end": true, "reason": "done" | "error" } }
```

## Verbs (proposal)

Concrete verb names are proposals. The categories are architectural.

### Configuration

- `compile_dry_run` — take a config directory path, return the compiled FlowGraph plus diagnostics. Pure; no side effects.
- `reload` — trigger re-read/re-compile/swap of the active config.
- `get_active_config` — return the current `MergedConfig` and compiled `FlowGraph` hash.

### Observability

- `list_connections` — snapshot of live connections (remote, local, transport, age, bytes, current node).
- `tail_flow_log` — stream flow-path events: `"conn X matched predicate Y at node Z, branched to A"`. One event per predicate evaluation or Terminator invocation.
- `tail_log` — stream the structured log.
- `get_metrics` — counter/gauge snapshot. The daemon's metrics backend is the [`metrics`](https://crates.io/crates/metrics) crate (facade), with `metrics-exporter-prometheus` recording into a registry exposed via `PrometheusHandle::render()`. `get_metrics` accepts `format: "prometheus" | "json"` in its args; default `"prometheus"` returns the standard text exposition format suitable for scraping. All counters/gauges defined by vane (error totals, pool events, latency histograms, rate-limit hits, WASM pool events) go through the `metrics::counter!` / `metrics::gauge!` / `metrics::histogram!` macros — no bespoke facade.

  **Exposure trade-off**: exposing metrics through the management verb means Prometheus scrapers must authenticate (bearer token on HTTP transport, file-permission boundary on Unix). The standard Prometheus scrape pattern (`GET /metrics` unauthenticated on a dedicated port) is intentionally **not** the default — vane treats metrics as privileged information. Operators who want a scrape-friendly endpoint can bridge via `curl -H "Authorization: Bearer $TOKEN" https://...` piped to a sidecar. A dedicated `VANE_METRICS_HTTP_BIND` is reserved for post-MVP if the scrape-first workflow becomes common.

### Runtime

- `stats` — daemon summary: uptime, active connections, FlowGraph version hash, WASM pool status.
- `shutdown` — graceful shutdown (drain, wait, exit).

### State

- `list_wasm_pools` — per stateful WASM module: pool size, in-use count, total allocations, failures.
- `list_upstreams` — pooled HTTP upstream connections (hyper-util client) and QUIC associations.

## Auth model

### Unix socket

No auth. `chmod 0660 /var/run/vaned.sock`, group `vaneadm`. Users in that group have full control.

### HTTP-over-TCP

- Bearer token in `Authorization: Bearer <token>`. Token hash stored in `config.json`.
- TLS required when bound to non-loopback. Refuse to start if misconfigured.

There is no per-verb RBAC. Management is root-level: you have access or you don't. Finer-grained auth is post-MVP.

## Idempotency

- `reload` is idempotent when the config has not changed (same version hash returned).
- `compile_dry_run` is pure.
- `list_*` are read-only snapshots.
- `shutdown` is not idempotent (runs once per daemon lifetime).

## Errors

Errors serialize from the `Error` shape in [`03-types.md`](03-types.md):

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
