# vane-engine

Source: [`crates/engine/`](../../crates/engine/).

The runtime and the linker. Implements `MiddlewareInst` / `FetchInst`, links a symbolic graph into an executable one, owns listener tasks, owns the executor.

TLS is in [`engine-tls.md`](engine-tls.md). ACME is in [`engine-acme.md`](engine-acme.md). WASM host integration is in [`engine-wasm.md`](engine-wasm.md). This file covers everything else.

## Owns

- **Runtime IR** — `FlowGraph` (linked form holding `Vec<MiddlewareInst>` and `Vec<FetchInst>` of trait objects), `MiddlewareInst`, `FetchInst`. Source: `flow_graph.rs`.
- **Link pass** — `FlowGraph::link(sym, mw_factories, fetch_factories)`. Where feature-availability rejection happens: a `SymbolicMiddlewareRef` for a kind the build disabled fails here, not in core. Source: `flow_graph.rs::link`.
- **Factories** — `MiddlewareFactories`, `FetchFactories`. Registries mapping `name` → constructor. Engine registers built-ins at startup; WASM factories come from `vane-wasm`. Source: `factories.rs`.
- **Metadata provider impls** — concrete `MiddlewareMetadataProvider` / `FetchMetadataProvider` the daemon passes into core's `compile`. Stateless / `needs_body` / `kind` come from the same registry so compile-time analysis and link-time construction agree.
- **Executor** — iterative walker. Source: `executor.rs`.
- **Listeners** — per-`(transport, addr)` accept loop with bind retry, cancellation, drain. Source: `listener.rs`, `listener_udp.rs`, `h3/listener.rs`.
- **Hot reload** — `ArcSwap<FlowGraph>` plumbing. Source: `hot_reload.rs`.
- **HTTP server integration** — hyper for H1/H2 (`upgrade.rs`), engine's `H3Body` + h3 path for H3 (`h3/body.rs`, `h3/listener.rs`).
- **Upstream fetch** — `HttpProxy`, `HttpSynthesize`, `WebSocketUpgrade`, `L4Forward`. Source: `fetch/`.
- **Built-in middleware** — `host_header_match`, `path_prefix`, `method_match`, `forward_client_ip`, `sni_peek`, `rate_limit`. Source: `middleware/`.
- **Protocol detect** — listener-side L4 peek that classifies TLS / H1 / H2 / QUIC / DNS / Unknown. Source: `protocol_detect.rs`.
- **DNS resolver** — `hickory-resolver` integration; per-upstream nameserver override. Source: `fetch/dns.rs`.
- **L1 security floor** — accept / pre-handshake / parse-time enforcement. Source: `security.rs`.
- **Flow log sink fan-out** — broadcast-channel-backed `FlowLogSink` impl with `RingBufferSink`, `FileSink`, `FanoutSink`. Source: `flow_log_sink/`.
- **Tracing** — `tracing-subscriber` init plus a broadcast-backed sink for `tail_log`. Source: `tracing_init.rs`, `tracing_broadcast.rs`.
- **Metrics** — `metrics` crate facade; `metrics-exporter-prometheus` wired here. Source: `metrics.rs`.

## Crate dependencies

`vane-core` + `tokio`, `hyper`, `hyper-util`, `hyper-rustls`, `h3`, `h3-quinn`, `quinn`, `quinn-proto` (for the multi-packet-peek ClientHello extraction path), `rustls`, `rustls-native-certs`, `tokio-rustls`, `hickory-resolver`, `dashmap`, `webpki`, `notify` + `notify-debouncer-full`, `metrics` + `metrics-exporter-prometheus`, `instant-acme` (gated by `acme`), `rand`, `libc` (gated by `cgi`).

## Listeners

`Listener` is `(transport, addr, kind)`. `kind` is derived at compile from each listener's FlowGraph entry subgraph — see [`core.md` § _Listener kind derivation_](core.md#listener-kind-derivation).

Bind, drain, IPv4/IPv6, partial-bind tolerance: see [`topology.md` § _Listener lifecycle_](../topology.md#listener-lifecycle).

### Protocol detection

Listener-side peek reads up to 8 KiB and classifies via built-in detectors. `PeekResult` populates `ConnContext.peek` and (for TLS) `ConnContext.tls.sni` before any middleware runs.

```rust
pub enum DetectedProtocol {
    TlsClientHello,
    Http1,
    Http2Preface,
    QuicInitial,
    Dns,
    Unknown,
}
```

Source: `crates/core/src/protocol_detect.rs` (types), `crates/engine/src/protocol_detect.rs` (detector + listener integration).

SNI normalization: parsed `server_name` is ASCII-lowercased before writing `ctx.tls.sni`. The same invariant holds at every other ingress (cert resolver, populator-time cert keys). The predicate compiler rejects uppercase SNI literals so the hot-path comparison stays byte-for-byte.

### Dispatch table

Combination of derived `ListenerKind`, `PeekResult.detected`, and whether a `rustls::ServerConfig` is bound to the address (graph-derived: `Some` iff at least one rule with a `tls` block lists this address):

| Kind   | `detected`               | `listener_tls` | Dispatch                                                                |
| ------ | ------------------------ | -------------- | ----------------------------------------------------------------------- |
| `Raw`  | any                      | any            | Hand `PeekedStream` (or raw stream when `needs_peek=false`) to L4 graph |
| `Http` | `TlsClientHello`         | `Some`         | TLS handshake → L7                                                      |
| `Http` | `TlsClientHello`         | `None`         | Reject — graph claims L7 but cannot decrypt                             |
| `Http` | `Http1` / `Http2Preface` | any            | Reject — `Http`-derived listeners do not accept cleartext               |
| `Http` | `Unknown`                | any            | Reject — opaque bytes have no L4 fallback                               |
| `Auto` | `TlsClientHello`         | `Some`         | TLS handshake → L7                                                      |
| `Auto` | `TlsClientHello`         | `None`         | L4 subgraph if reachable; else reject (SNI passthrough path)            |
| `Auto` | `Http1`                  | any            | Cleartext H1 directly                                                   |
| `Auto` | `Http2Preface`           | any            | Cleartext H2 (h2c) directly                                             |
| `Auto` | `Unknown`                | any            | L4 subgraph if reachable; else reject                                   |

Listeners whose graph has no reachable `L4Peek` skip the peek prelude entirely (`needs_peek = false`); the `(kind, listener_tls)` decision suffices. `Http+None` with no peek reaches `Node::Upgrade` and drives the cleartext stream through `hyper::server::conn::http1::Builder` — the cleartext HTTP reverse-proxy path. Non-HTTP traffic on this path fails closed via hyper's parse error rather than via an explicit pre-protocol reject.

`Auto` listeners always have at least one reachable `L4Peek` by construction.

Rejection closes TCP without writing application bytes. `tracing::debug!` records the reason; the L1 floor still counts the connection.

### `udp_dispatch`

Vane owns the physical UDP socket and demultiplexes packets to one of: an `L4Forward` session, a QUIC virtual socket (per-`Http`-UDP-listener `quinn::Endpoint`), or — for cold-path packets — into the FlowGraph entry path.

Hot path: dispatch table lookup, push to forwarder mpsc or QUIC endpoint inbound.

Cold path: classify by packet form. QUIC long-header packets on `Http` UDP listeners short-circuit FlowGraph entry — `quinn::Endpoint` handles handshake / streams / migration internally; vane only routes datagrams in. Other UDP cold-path datagrams build `L4Conn::Udp(UdpAssoc { socket, peer, first_packets })` and enter the graph.

Vane does not track QUIC connection IDs in its dispatch table. CID demultiplexing happens entirely inside `quinn::Endpoint`. The earlier per-connection design that subscribed to `quinn-proto`'s endpoint-event stream is not implementable against the public API (`EndpointEvent` is `pub(crate)`-wrapped, exposing only `is_drained()`).

UDP idle timeout is single-authority. `L4Forward.idle_timeout` is the only timer for forwarder sessions; QUIC idle timing is `quinn`'s own knob. No `min(a, b)` races.

Source: `listener_udp.rs`.

### Multi-packet peek

QUIC ClientHellos may span multiple Initial packets when carrying large key shares (PQ hybrids push past one MTU). The single-datagram `first_packet` model is sufficient for L4 forwards keyed on peer 4-tuple and for H3 termination (where `quinn` reassembles internally). It is not sufficient for QUIC SNI passthrough — vane must extract SNI from a possibly-fragmented ClientHello.

The cold path supports a third state, **pending-peek**, between miss and active dispatch: datagrams accumulate, the parser tries SNI extraction on each arrival, FlowGraph entry is delayed until SNI is known or a bound is exceeded.

Activation: compile-time, similar to `needs_peek`. Required iff any rule on the listener uses `tls.sni` and reaches an `L4Forward` terminator. H3-only termination does not require pending-peek.

Bounds (fixed):

| Bound                                        | Value | Rationale                                                              |
| -------------------------------------------- | ----- | ---------------------------------------------------------------------- |
| Max bytes per pending session                | 16 KB | Real ClientHellos rarely exceed 8 KB even with PQ; 16 KB has headroom. |
| Max datagrams per pending session            | 8     | Worst-observed real fragmentation is 2–3 packets.                      |
| Pending session lifetime                     | 1 s   | Covers cross-continent RTT; longer suggests attack or bug.             |
| Max concurrent pending sessions per listener | 1024  | Resource cap against floods. New sessions past the cap drop silently.  |

Pending entries are keyed by peer 4-tuple. QUIC peer migration only happens after handshake completes — NAT collisions during the same millisecond on the same public mapping are vanishingly rare; the cost is one retried handshake.

Initial packet payloads are encrypted with a key derived from the connection's initial Destination Connection ID (RFC 9001 §5.2), so any party with the DCID can decrypt — no secret material required. Vane delegates parsing to the workspace's [`clienthello`](clienthello.md) crate. Replay to handler: matched terminator is `L4Forward`; the forwarder sends every buffered datagram in original order before subscribing to the inbound mpsc.

QUIC v2 (RFC 9369) is mechanical to add (different initial salt + TLS 1.3 cipher suite) and not implemented in `clienthello` 0.1.0.

## Executor

Walker semantics, ownership invariants, `ExecutorOutput` shape, and `Terminator::Close` wire-level manifestation: see [`flow-model.md` § _Executor_](../flow-model.md#executor).

Source: `executor.rs`.

The walker emits one `tracing::trace!` per loop iteration and one `FlowLogEvent` per step under `Debug` verbosity (always one `Trajectory` event per request under `Trajectory` verbosity). The L7 path's `ExecutorOutput::HttpResponse(r)` flows back through the hyper service-fn at `Node::Upgrade`, which serialises onto the wire.

## Fetch

Fetch is the upstream-contact node. Every flow has at most one Fetch. Fetch is built into `vaned`; not extensible.

```rust
#[async_trait]
pub trait L7Fetch: Send + Sync {
    async fn fetch(&self, req: Request, conn: &Arc<ConnContext>, ctx: &mut FlowCtx<'_>)
        -> Result<L7FetchOutput, Error>;
}

#[async_trait]
pub trait L4Fetch: Send + Sync {
    async fn fetch(&self, l4: L4Conn, conn: &Arc<ConnContext>, ctx: &mut FlowCtx<'_>)
        -> Result<Tunnel, Error>;
}
```

`Request` and `L4Conn` are owned (consumed). The type system enforces "Fetch is the terminal owner of the request phase" — after `L7Fetch::fetch` returns, no caller can reach the old `Request`.

Source traits: `crates/core/src/fetch.rs`. Implementations: `crates/engine/src/fetch/`.

### Concrete fetches

| Variant                 | Source                       | Notes                                                                                       |
| ----------------------- | ---------------------------- | ------------------------------------------------------------------------------------------- |
| `HttpProxyFetch`        | `fetch/http_proxy.rs`        | Core reverse proxy. H1/H2/H3 client × upstream, all 9 combinations.                         |
| `HttpSynthesizeFetch`   | `fetch/http_synthesize.rs`   | Fabricate `Response` directly. No upstream contact.                                         |
| `WebSocketUpgradeFetch` | `fetch/websocket_upgrade.rs` | H1.1 ↔ H1.1 byte tunnel, bi-outcome 101 vs 4xx.                                             |
| `L4ForwardFetch`        | `fetch/l4_forward.rs`        | TCP `copy_bidirectional` (uses `splice(2)` on Linux) or UDP 5-tuple.                        |
| `AcmeChallengeFetch`    | `fetch/acme_challenge.rs`    | High-priority `/.well-known/acme-challenge/` synth, see [`engine-acme.md`](engine-acme.md). |

`HttpProxyFetch` reads the request body via `http_body::Body::poll_frame`, hands each `Bytes` directly to the upstream encoder. For H3 upstream, `Frame::data(Bytes)` becomes `h3::client::RequestStream::send_data` ownership-transfer — h3 accepts `impl Buf`. Trailers map to `send_trailers(HeaderMap)`. The reverse direction drives `H3Body` through `poll_frame`.

`HttpProxyFetch` commits to streaming upstream response bodies — wraps in `Body::Stream(Box::pin(...))`. Never collects defensively. `HttpSynthesizeFetch` always produces `Body::Static` by construction.

WebSocket close-frame semantics: vane is a byte tunnel after upgrade. It does not synthesize or interpret `Close` frames. RFC 6455 §7.1.5 explicitly allows the abnormal-closure case (FIN without Close); applications must tolerate it. Matches haproxy / envoy tunnel behavior. Parsing frames to synthesize Close would re-introduce the frame-aware path that `ByteTunnel`-by-design rejects.

### Variant ergonomics in config

JSON `"type"` aliases (full table at `crates/core/src/rule.rs`, runtime mapping at `crates/engine/src/factories.rs`):

| `type`                                                       | Concrete impl                                                                         |
| ------------------------------------------------------------ | ------------------------------------------------------------------------------------- |
| `tcp_forward`                                                | `L4ForwardFetch { transport: Tcp }`                                                   |
| `udp_forward`                                                | `L4ForwardFetch { transport: Udp }`                                                   |
| `http_proxy` / `http1_proxy` / `http2_proxy` / `http3_proxy` | `HttpProxyFetch`, version: Auto / Http1 / Http2 / Http3                               |
| `unix_proxy`                                                 | `HttpProxyFetch { upstream: Unix }`                                                   |
| `cgi`                                                        | `HttpProxyFetch { upstream: Cgi }`                                                    |
| `websocket`                                                  | `WebSocketUpgradeFetch`                                                               |
| `static`                                                     | `HttpSynthesizeFetch`                                                                 |
| `redirect_https`                                             | `HttpSynthesizeFetch { status: 308, headers: { location: "https://${host}${uri}" } }` |

Aliases are sugar; new aliases are parser changes, not new `FetchKind` variants.

### Retry

Lives inside Fetch. Configured per-rule:

- `max_attempts` — default 1 (no retry).
- `methods` — idempotent whitelist (GET / HEAD / PUT / DELETE / OPTIONS by default); POST / PATCH require explicit opt-in.
- `backoff` — `"none"` / `{ "fixed": "<duration>" }` / `"exponential"` / `{ "exponential": { "base", "max", "jitter" } }`. Default exponential, base 100 ms, max 5 s, full jitter.
- `buffering` — `"opportunistic"` (default) or `"force"`.

The retry decision consumes `Error::is_retryable()` — single source of truth, defined in [`core.md` § _Error type_](core.md#error-type).

Retry buffering: `Body::Stream` is one-shot (hyper Incoming, H3Body cannot be replayed). Retry only proceeds when the body is `Body::Static` or `Body::Empty`.

| `max_attempts` | `buffering`     | Behavior                                                                                                                                                                                                                                           |
| -------------- | --------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `1`            | (irrelevant)    | Retry off; body posture decided by other rules (LazyBuffer track).                                                                                                                                                                                 |
| `> 1`          | `opportunistic` | Default. `lower` does not flag this fetch as a buffering trigger. If a different node forces buffering, retry sees `Body::Static`. If body reaches Fetch as `Body::Stream`, first failure returns immediately — no retry, no surprise memory cost. |
| `> 1`          | `force`         | `lower` flags the fetch's incoming edge with `collect_body_before = Some(BodySide::Request)`; body always arrives as `Body::Static`.                                                                                                               |

Retry scope is one `HttpUpstream`. Multi-upstream failover is multiple rules with fallback predicates, not retry config.

Source: `fetch/retry.rs`.

## Upstream pools

Two daemon-level pool systems, one per transport family:

```
vaned
├─ TcpPool   (singleton; hyper-util Client with ConnectorStack)
│   └─ per-destination cache, keyed by TcpFingerprint
└─ QuicPool  (singleton; per-destination h3 client manager)
    └─ per-destination QUIC connection, keyed by QuicFingerprint
```

TCP pooling is `hyper_util::client::legacy::Client`'s; vane keys per-`(version, tls)` slot. QUIC is per-destination — each upstream gets one `h3::client::Connection` multiplexing many streams. Source: `fetch/upstream.rs`, `fetch/quic_pool.rs`, `fetch/client_cache.rs`.

QuicPool socket model: each entry owns its own ephemeral `tokio::net::UdpSocket` — `quinn::Endpoint::client(addr, ...)` binds a fresh client port per entry. Two upstreams with the same fingerprint share one entry (and one socket). A daemon-wide shared client UDP socket multiplexing every upstream connection is not used — port-pressure savings are small in realistic deployments and the implementation cost (vane-side CID-based outbound routing on top of quinn) is significant.

No 0-RTT to upstream. Replay-risk trade-off documented in [`engine-tls.md` § _0-RTT_](engine-tls.md#tls-13-0-rtt-early-data) is operator-opt-in only because the operator owns both the rule and the application. Upstream replay safety is owned by the upstream operator, who has not granted vane that authority. Every upstream H3 dial is full 1-RTT.

### Pool fingerprint

Two fetches share a slot iff their fingerprints match:

```
TcpFingerprint  = (addr, version_slot, tls_hash)
QuicFingerprint = (addr, tls_hash)

version_slot = Auto | Http1 | Http2 | Http3
tls_hash     = hash(root_CA_source, client_cert, crls, verify_mode, alpn_protocols)
```

`verify_mode` and `crls` participate — Full vs Skip and different CRL sets are distinct trust boundaries.

CRL fingerprint = source identity, not content. `CrlSource::File(path)` hashes the path string; `CrlSource::Url(url)` hashes the URL. Fetched CRL bytes are not part of the fingerprint. Refreshes update `rustls`'s CRL provider in place; new handshakes see fresh revocation immediately while in-flight TLS connections keep serving (a revoked cert caught by a fresh CRL affects new handshakes, which is correct — established sessions completed identity verification at handshake time).

### Lifetime

Daemon-level. FlowGraph reload does not touch pools. Connection-level lifetime is `idle_timeout` (default 30s). When reload removes the last rule referencing some upstream, its connections idle out naturally over the next 30 seconds.

Reference-counted pools (last-dropped Fetch closes the pool) are deliberately not used — complexity exceeds the benefit; the cost is transient idle connections for a few seconds after reload. `pool.drain <fingerprint>` mgmt verb provides forced removal — useful after cert rotation.

### Exhaustion defaults (per upstream)

| Parameter                 | Default | Meaning                                         |
| ------------------------- | ------- | ----------------------------------------------- |
| `max_idle_per_host`       | 32      | Cap on idle connections per authority           |
| `max_concurrent_per_host` | 100     | Cap on concurrent in-flight per authority       |
| `connect_timeout`         | 5 s     | Bound on establishing a new upstream connection |
| `idle_timeout`            | 30 s    | Idle close threshold                            |

Saturation: new requests wait internally; wait exceeding `connect_timeout` returns `Upstream` → 503.

### DNS

`hickory-resolver` with per-upstream nameserver override. Default reads OS DNS settings. Override is strict (no fallback to system) — split-horizon deployments would consider fall-through a leak. Override order is preserved end-to-end; primary/secondary intent is honored.

Source: `fetch/dns.rs`.

### Stream fan-out / fan-in

H2/H3 client → H1 upstream: pool opens multiple TCP connections up to `max_concurrent_per_host`. H1 client → H2/H3 upstream: each H1 request becomes one H2/H3 stream on the shared upstream connection.

### Error classification

`is_retryable` table lives at `crates/core/src/error.rs`. The H2/H3 `GOAWAY` case (`UpstreamReason::Gone`) is retry-eligible regardless of method idempotency — upstream explicitly said "did not process this stream", replay is safe. `Response 4xx/5xx` is not retry-eligible — those are valid responses, retrying duplicates the request without basis.

## Body streaming

Vane streams bodies end-to-end iff neither LazyBuffer track fires on the path (see [`flow-model.md` § _LazyBuffer_](../flow-model.md#lazybuffer)).

| Combo                     | Streaming posture                                                                                                                     |
| ------------------------- | ------------------------------------------------------------------------------------------------------------------------------------- |
| H2 ↔ H2, H3 ↔ H3, H2 ↔ H3 | Native bidirectional streaming. Multiplexed transports support partial-frame handoff.                                                 |
| H1.1 ↔ {H1.1, H2, H3}     | Half-duplex per direction; client-side reuse requires H1.1 keep-alive.                                                                |
| H1.0 client ↔ any         | Client side is one-shot — `Connection: keep-alive` on H1.0 is rarely honored. After `WriteHttpResponse`, client TCP closes.           |
| Any ↔ CGI                 | Upstream cannot stream the request side (RFC 3875 requires stdin EOF before child output). Response side streams as the child writes. |

LazyBuffer firing on a side degrades that side to buffered. The other side remains streaming. The executor uses the same body sink/source regardless of posture.

H1 egress framing: `Body::Stream(_)` → `Transfer-Encoding: chunked` (strips any `Content-Length`). `Body::Static(_)` → `Content-Length`. `Body::Empty` → no body, no framing header. The encoder owns this; middleware never does.

## Middleware

Built-ins live in `middleware/`. Each is a `struct` implementing one of the four traits from [`core.md`](core.md):

- `host_header_match.rs` — reads `http.header.host`.
- `path_prefix.rs` — reads `http.uri.path`.
- `method_match.rs` — reads `http.method`.
- `forward_client_ip.rs` — sets `X-Forwarded-For` (append) and `X-Real-IP` (overwrite) from `ConnContext.remote`. Default header set `["x-forwarded-for", "x-real-ip"]`. Disabled at the raw-rule layer; `reverse_proxy` preset enables.
- `sni_peek.rs` — reads ClientHello via `rustls::server::Acceptor`, populates `ctx.tls.sni`.
- `rate_limit.rs` — token bucket per [`core.md` § _Rate limit_](core.md#rate-limit-l2).

Stateless middleware is hash-consed by `(name, canonical_args_json)` per [`flow-model.md` § _Hash-consing_](../flow-model.md#hash-consing). Stateful is per-call-site by construction.

WASM-backed middleware integration (`MiddlewareInst::Wasm`) is in [`engine-wasm.md`](engine-wasm.md).

## Security floor (L1)

Daemon self-preservation. Always present, values configurable upward, floors enforced at compile.

Enforced at listener accept, pre-handshake, header parse — before any FlowGraph walk. Architecturally outside user rules. Source: `security.rs`.

| Limit                            | Default  | Floor   | Layer         | Trigger            |
| -------------------------------- | -------- | ------- | ------------- | ------------------ |
| `max_header_bytes`               | 64 KiB   | 4 KiB   | L7 parse      | close + 400        |
| `max_headers_count`              | 100      | 20      | L7 parse      | close + 400        |
| `header_timeout`                 | 30 s     | 5 s     | ingress       | close              |
| `body_idle_timeout`              | 30 s     | 5 s     | L7 body       | close + 408        |
| `max_concurrent_conns_per_ip`    | 100      | 10      | accept        | reject new conn    |
| `max_handshake_rate_per_ip`      | 10 / s   | 1 / s   | pre-handshake | TCP reset          |
| `max_in_flight_streams_per_conn` | 100      | 10      | H2 / H3       | RST_STREAM         |
| `max_request_rate_per_conn`      | 1000 / s | 100 / s | H2 / H3       | RST_STREAM / close |
| `max_total_connections`          | 65536    | 1024    | accept        | reject new conn    |
| `max_pending_handshakes`         | 1000     | 100     | TLS accept    | reject new conn    |

Default-calibration target: normal traffic on moderate sites should never trigger any of these; triggering means misbehaving client or attack.

Observability: structured log dedups by `(limit, source_ip)` within a 1-second window — one line per attack path per second. Flow log emits `FlowLogKind::SecurityLimit` with the same dedup. Metrics counter `vane.security.limit_hit_total{limit, source}` is full-fidelity (designed to absorb high cardinality).

Configured via `VANE_SEC_*` env vars. Daemon restart required to change. Not in `config.json` or `rules/*.json` — these describe the daemon's existence, not the flows it serves.

## CGI

Sole non-socket-based upstream. Per-request fork-exec via `tokio::process::Command`. Source: `fetch/cgi.rs`.

Process model:

- `env_clear()` then `envs(computed_rfc3875_vars)` — daemon env not inherited.
- `current_dir(working_dir)`.
- Stdio piped on all three.
- `pre_exec` closure runs after fork, before exec — issues async-signal-safe `setgid` / `setuid` / `setrlimit` / optional `chroot`. Errors return as `io::Error` to the parent's `spawn()`.
- `spawn()`, write request body to stdin, read child stdout, parse RFC 3875 response, wait for exit, clean up fds.

`pre_exec` requires `unsafe`. The workspace lints forbid `unsafe_code`; the CGI module carries a reviewed `#[allow(unsafe_code)]` documenting the async-signal-safety discipline of the closure body — no allocations, no mutex locks, no file I/O beyond the listed syscalls.

Cost: fork + exec is ~1 ms on Linux plus the binary's own startup. Operators opt in deliberately.

### Environment

Constructed explicitly. RFC 3875 required vars (`CONTENT_LENGTH`, `REQUEST_METHOD`, `SERVER_PROTOCOL` always `"HTTP/1.1"` even when client is H2/H3 — we downgrade, etc.) plus common extensions (`REMOTE_PORT`, `REQUEST_URI`, `REQUEST_SCHEME`, `HTTPS`, `DOCUMENT_URI`).

`HTTP_<UPPERCASE_HEADER>` vars are filtered by per-rule `block_headers`. The list is required (no implicit default); CLI/TUI emits `["Authorization", "Cookie", "Proxy-Authorization"]` so operators see what is being blocked rather than discovering it in source. `Authorization` and `Cookie` carry credentials whose appearance in `/proc/<pid>/environ` and child sub-processes' envs leaks them.

User-provided `env` entries merge for non-reserved keys. Reserved set is the union of every RFC 3875 var, every common-extension var, every `HTTP_*` form. Overlap is a compile error — vane computes these per request from connection state; silent overrides produce CGI scripts reading confidently-wrong data.

Daemon's own env (loaded by `dotenvy` at startup, including secrets) is not propagated to CGI children. Boundary keeps CGI scripts from accidentally reading daemon secrets.

### Path

Explicit `script_name` field splits URI into `SCRIPT_NAME` / `PATH_INFO` / `QUERY_STRING`. No filesystem walking (which is how Apache mod_cgi does it; fragile).

For request `GET /cgi-bin/app.cgi/users/42?sort=asc` with rule `script_name: "/cgi-bin/app.cgi"`:

```
SCRIPT_NAME   = /cgi-bin/app.cgi
PATH_INFO     = /users/42
QUERY_STRING  = sort=asc
```

If the request URI does not begin with `script_name`, the rule should not match (path-prefix predicate on `script_name` is typically the right predicate; rule author's responsibility).

### Streaming posture

Half-buffered by RFC 3875 constraint:

- Request side — vane writes body to child stdin as bytes arrive; child must see stdin EOF before producing output (typical CGI scripts read stdin fully before writing). Observationally equivalent to "request buffered at the child". `max_body_size` enforced during write; exceeding sends `SIGTERM` and returns 413.
- Response side — child stdout reads frame by frame after the RFC 3875 header block (`\r\n\r\n`). Each `read()` becomes a `Body::Stream` frame. No vane-side buffering.

LazyBuffer sees the CGI Fetch like any other: response-side `needs_body` middleware buffers as usual.

Retry on CGI is not supported — child is a one-shot process by RFC 3875. "Replay" would require re-forking, which changes the PID and breaks any PID-keyed external state.

Special headers: `Status: 200 OK` sets HTTP status code. `Location: /other` without `Status` sets 302. Other headers pass through. Body bytes begin after `\r\n\r\n` and continue until child closes stdout. Exit code: 0 → response from stdout; non-zero → 502, exit code logged.

### Security

Per-rule, enforced at spawn:

```rust
pub struct CgiSecurity {
    pub uid:    u32,                // required; setuid
    pub gid:    u32,                // required; setgid
    pub limits: ResourceLimits,
    pub chroot: Option<PathBuf>,    // schema reserved; runtime unimplemented
}

pub struct ResourceLimits {
    pub memory_mb:     Option<u64>,  // RLIMIT_AS;     null = no limit
    pub cpu_seconds:   Option<u64>,  // RLIMIT_CPU;    null = no limit
    pub max_processes: Option<u64>,  // RLIMIT_NPROC;  null = no limit
}
```

`uid`, `gid`, and every `ResourceLimits` field are required. CLI/TUI defaults: `memory_mb: 256`, `cpu_seconds: 30`, `max_processes: null`. Absence is a compile error — operators must consciously decide whether each limit applies.

If resolved `uid` is `0` at boot, vane emits `WARN` (`"cgi rule '<name>' configured to run as root; verify this is intended"`) but does not refuse to start — container deployments where the daemon's view of root is namespaced commonly use uid 0 legitimately.

`uid` / `gid` switching requires `CAP_SETUID` / `CAP_SETGID` on the daemon. Without them, spawn fails with a specific error.

`rlimits` enforced by the kernel — exceeding kills the child; non-zero exit; 502 to client.

`chroot` schema field is reserved. A rule with `chroot: Some(...)` fails compile with `"chroot is reserved but not yet implemented"`. Locking the field shape now keeps the JSON schema stable for the future implementation pass.

### Concurrency cap

Daemon-global `max_concurrent_cgi_processes`, default 100, configurable via `VANE_CGI_MAX_CONCURRENT`. At cap, new CGI requests return 503 immediately — no queueing. Queueing under sustained overload amplifies pressure (each queued request still holds connection + request state).

### stderr

Consumed line-by-line. Each line emits a `tracing::warn!` with structured fields (`event.target = "vane::cgi"`, `rule`, `binary`, `pid`, `message`). Operators filter via `tail_log` with `event.target != "vane::cgi"`.

### Bootstrap validation

`vane compile` validates each CGI rule's `binary` path:

- Path must exist.
- File must be executable by the configured `uid` (`access(2)` with `X_OK`).

Failures are rule-level compile errors, not daemon-wide boot failures. Network-mounted binaries that may be temporarily unavailable at startup either mount before reload, or the operator deals with the rule-level error and reloads again.

## Hot reload

`ArcSwap<FlowGraph>` swaps at the granularity of the whole graph. File watcher (`notify` + `notify-debouncer-full`, default 250 ms debounce) observes `<config-dir>/` and triggers re-merge → re-compile → swap.

Watcher arm-up is strict-ordered — listeners must be running before `spawn_watcher` registers, so a reload event raced ahead of listener bind has nothing useful to do. If `notify` registration fails (typically permission-denied), the daemon logs a warning and continues without auto-reload; reload is then driven by the `vane reload` mgmt verb.

Watched events are filtered to file-level mutations under the watched tree (created / content modified / deleted). Editor swap-file dance and other notify noise is tolerated because the post-reload `version_hash` idempotency check skips the `ArcSwap::store` when content is semantically unchanged.

NodeId stability is not preserved across reloads — `lower` numbers nodes in iteration order, and a single edited rule shifts every later allocation. Listener accept loops therefore must not capture a boot-time `NodeId` and reuse it. Each accept loads the current graph (`ArcSwap::load_full()`) and looks up its entry by `local_addr`. If the address is no longer present in the active graph (rule deleted, listener about to be torn down by reconcile), the connection closes — TCP RST, same as a no-rule listener at boot.

Listener-set diff happens immediately after the successful `ArcSwap::store` via `ListenerSet::reconcile`. Addresses present in the new graph but not currently bound spawn fresh accept loops; addresses present but absent from the new graph fire `accept_cancel`, drain in the background up to 30 s, then escalate to `force_cancel`.

Source: `hot_reload.rs`.

## Tests

Integration coverage in `crates/engine/tests/`:

- `executor.rs`, `link.rs`, `listener*.rs`, `protocol_detect.rs`, `udp_forward.rs`, `sni_peek.rs`.
- `fetch_*` covers each Fetch variant including retry, mTLS, H3 paths, DNS overrides.
- `flow_log_sink.rs`, `ticketer.rs`, `crl_fetch.rs`, `ocsp_e2e.rs`.
- `acme_*_e2e.rs` are gated behind the `acme` feature; HTTP-01 paths spin up Pebble via `testcontainers`, DNS-01 paths use `vane-testutil::mock_dns()`.
- `wasm_http_fetch.rs`, `wasm_l4_bytes_tcp.rs` are gated behind the `wasm` feature.
- `middleware_*.rs` covers each built-in middleware.
- `hyper_upgrade.rs` covers the L4→L7 bridge.
