# Fetch and Terminator

The transition from request to response happens in **Fetch**. The final write-to-client and connection close happens in **Terminator**. Both are always built into `vaned`; neither is extensible through WASM or any other mechanism.

## Why built-in only

Fetch holds upstream socket handles, spawns subprocesses, drives TLS, manages connection pools. Terminator writes to the client socket and decides the connection's fate. These require host-level capabilities (syscalls, filesystem, network identity) that the WASM sandbox is designed to deny.

Every prior attempt to make these extensible (v1's subprocess plugin model) leaked the same way: sandbox pierce, divergent error semantics, duplicate pool/TLS code per plugin. The decision "Fetch and Terminator are built-in forever" is permanent.

WASM extends **decisions** (Middleware). Fetch and Terminator are **actions**; actions stay in trusted Rust.

---

## Fetch

Fetch is the upstream-contact node. It is the only step in any flow that touches an external network or process. Every flow has at most one Fetch.

Contract:

- **L7 Fetch** consumes the `Request` (including request-body ownership) and produces a `Response`.
- **L4 Fetch** consumes the `L4Conn` and establishes a byte tunnel.
- **Failure is typed** — upstream unreachable, timeout, malformed response, pool exhaustion: each is an `ErrorKind` that flows into the rest of the pipeline.

### Trait surface

Fetch splits into two traits along the L4 / L7 phase axis. The owned-parameter shape is load-bearing: `Request` and `L4Conn` are **consumed** by Fetch, not borrowed. This is how the type system enforces "Fetch is the terminal owner of the request phase" — after `L7Fetch::fetch` returns, no caller can reach the old `Request` (Rust's ownership rules; not a convention).

```rust
#[trait_variant::make(L7Fetch: Send)]
pub trait L7FetchLocal {
    async fn fetch(
        &self,
        req:  Request,                 // owned — consumed by the upstream client or synthesis
        conn: &Arc<ConnContext>,
        ctx:  &mut FlowCtx<'_>,
    ) -> Result<L7FetchOutput, Error>;
}

#[trait_variant::make(L4Fetch: Send)]
pub trait L4FetchLocal {
    async fn fetch(
        &self,
        l4:   L4Conn,                  // owned — becomes one end of the tunnel
        conn: &Arc<ConnContext>,
        ctx:  &mut FlowCtx<'_>,
    ) -> Result<Tunnel, Error>;
}

pub enum L7FetchOutput {
    Response(Response),                // HttpProxy, HttpSynthesize, WebSocketUpgrade-on-non-101
    Tunnel(Tunnel),                    // WebSocketUpgrade-on-101
}

// Lives in vane-engine, not vane-core — same reason as MiddlewareInst:
// Arc<dyn _> values only engine can construct. Core's IR references
// Fetch by SymbolicFetchRef { kind, args }; the link pass instantiates
// FetchInst during FlowGraph::link.
pub enum FetchInst {
    L7(Arc<dyn L7Fetch>),              // HttpProxy, HttpSynthesize, WebSocketUpgrade
    L4(Arc<dyn L4Fetch>),              // L4Forward
}

pub struct Tunnel {
    pub client:   Pin<Box<dyn AsyncReadWrite + Send>>,
    pub upstream: Pin<Box<dyn AsyncReadWrite + Send>>,
    pub close_reason_tx: Option<tokio::sync::oneshot::Sender<CloseReason>>,
}

/// Convenience trait-alias so `Box<dyn AsyncReadWrite>` compiles cleanly.
pub trait AsyncReadWrite: tokio::io::AsyncRead + tokio::io::AsyncWrite {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + ?Sized> AsyncReadWrite for T {}
```

The executor (`02-flow.md`) dispatches first on `FetchInst::L4 | L7`, then on the output variant of `L7Fetch` (`Response` vs `Tunnel`) to pick the `next_response` or `next_tunnel` edge on the Fetch node. The phase state machine (also in `02-flow.md`) guarantees that `L4Fetch` (i.e. `L4ForwardFetch`) always sits on an L4 path with `next_tunnel` set, and `WebSocketUpgradeFetch` has both edges set — the core validator fails the compile otherwise.

`L7Fetch::fetch` consuming `req: Request` also answers "what happens to the request body on a response-middleware path": the request no longer exists. Response-phase middleware works against `&mut Response` only; if it needs request-derived data, request-phase middleware stashes it in `ConnContext.user` (see `04-middleware.md`).

### Where the types live

| Type                                                    | Crate         | Why                                                  |
| ------------------------------------------------------- | ------------- | ---------------------------------------------------- |
| `L7Fetch`, `L4Fetch`, `L7FetchOutput`, `Tunnel` traits  | `vane-core`   | Trait contracts are part of the IR                   |
| `FetchInst`                                             | `vane-engine` | Holds `Arc<dyn _>` that only engine constructs       |
| `SymbolicFetchRef { kind, args }`                       | `vane-core`   | The IR reference; output of `lower`                  |
| `FetchKind` (enum: `L4Forward` / `HttpProxy` / ...)     | `vane-core`   | Symbolic discriminant; drives phase table entries    |
| `FetchMetadataProvider` trait                           | `vane-core`   | Compile-time uses it for kind / output-mode metadata |
| Built-in `L7Fetch` / `L4Fetch` impls + factory registry | `vane-engine` | Registered at daemon startup, consulted by `link`    |

The same `SymbolicFlowGraph` can be re-linked against a different factory set — useful for test harnesses that want to intercept Fetch, without rebuilding the IR.

### Concrete Fetch kinds

Four built-in Fetch implementations, each a concrete struct that implements either `L7Fetch` or `L4Fetch`:

```rust
// Implement L7Fetch (live in vane-engine)
pub struct HttpProxyFetch        { upstream: HttpUpstream,    timeouts: Timeouts }
pub struct HttpSynthesizeFetch   { status:   StatusCode,      headers: HeaderMap, body: Bytes }
pub struct WebSocketUpgradeFetch { upstream: WsUpstream,      subprotocol: Option<String> }

// Implements L4Fetch (lives in vane-engine)
pub struct L4ForwardFetch        { upstream: SocketAddr,      transport: Transport, keep_alive: bool, idle_timeout: Duration }

// Core-side symbolic forms (live in vane-core; no runtime impls).

pub enum FetchKind {
    HttpProxy,
    HttpSynthesize,
    WebSocketUpgrade,
    L4Forward,
}

pub struct SymbolicFetchRef {
    pub kind: FetchKind,
    pub args: serde_json::Value,         // canonical-JSON form; opaque to core
}

pub struct FetchMetadata {
    pub kind:          FetchKind,
    pub phase:         FetchPhase,       // L4 | L7 — drives phase-state-machine placement
    pub output_modes:  FetchOutputModes, // { response: bool, tunnel: bool } — e.g. WS has both
    pub validate_args: fn(&serde_json::Value) -> Result<(), Error>,
}

pub trait FetchMetadataProvider {
    fn get(&self, kind: FetchKind) -> Option<FetchMetadata>;
}

pub enum HttpUpstream {
    Tcp  { addr: SocketAddr, version: UpstreamVersion, tls: Option<UpstreamTls> },
    Unix { path: PathBuf,    version: UpstreamVersion },
    Cgi  {
        binary:      PathBuf,
        script_name: String,                  // required URL prefix; no filesystem walk
        env:         Vec<(String, String)>,   // user-declared env, merged with computed RFC 3875 vars
        security:    CgiSecurity,             // uid/gid/rlimits — see 15-cgi.md
        working_dir: Option<PathBuf>,         // defaults to binary's parent dir
    },
}

pub enum UpstreamVersion { Auto, Http1, Http2, Http3 }

pub enum WsUpstream {
    Tcp  { addr: SocketAddr, tls: Option<UpstreamTls> },  // ws:// or wss://
    Unix { path: PathBuf },
}
```

### `HttpProxy`

The core reverse-proxy Fetch. Takes `Request`, returns `Response`.

**HTTP version any-combo invariant**: the (client HTTP version) × (upstream HTTP version) matrix is fully supported. All nine combinations work.

```
              upstream →  H1    H2    H3
client ↓
H1                        ok    ok    ok
H2                        ok    ok    ok
H3                        ok    ok    ok
```

**TLS is orthogonal on both sides.** `HttpUpstream::Tcp.tls` independently controls upstream encryption; client-side TLS termination is controlled by the listener.

```
              upstream →  HTTP   HTTPS
client ↓
HTTP                      ok     ok
HTTPS                     ok     ok
```

Implementation:

- Client-side HTTP version is fixed at L4→L7 upgrade (ALPN decides H1 vs H2; H3 is a separate path via `h3`).
- Upstream uses `hyper-util`'s `Client` for H1/H2 (ALPN-negotiated, connection-pooled) or `h3::client` for H3.
- Version translation (`Host` ↔ `:authority`, chunked ↔ DATA frames) is owned by the corresponding client/server library. Vane does not touch pseudo-headers.

### `HttpSynthesize`

Fabricates a Response directly. No upstream contact.

Use cases: health-check endpoints, default-deny responses, trivial static content. Serving actual file trees is out of scope for MVP (would be a new variant).

### `WebSocketUpgrade`

HTTP/1.1 WebSocket bridge. WebSocket over H2 (RFC 8441) and H3 are permanently out of scope; the incoming request must be H1.1, and the upstream must also be H1.1.

**Bi-outcome Fetch**: `WebSocketUpgrade` is unique among Fetch variants in that its output depends on the upstream's actual response:

- **Upstream responds `101 Switching Protocols`** → produces `Tunnel`; flow continues via `Node::Fetch.next_tunnel` to `Terminator::ByteTunnel`. Bidirectional byte copy until either side closes.
- **Upstream responds anything else** (400, 426, 5xx, normal HTTP response) → produces `Response`; flow continues via `Node::Fetch.next_response` through any response-phase middleware to `Terminator::WriteHttpResponse`. Upstream's response is transparently relayed to the client.
- **Upstream unreachable / handshake fails on our side** (network error, timeout, TLS failure) → produces `Response` with `502 Bad Gateway` or `504 Gateway Timeout`.

This lets vane handle the "client sent malformed WebSocket, upstream correctly rejected" case as transparent passthrough, while also handling the "our side couldn't reach upstream" case as a 5xx from vane.

**WS/WSS any-combo invariant**: the (client scheme) × (upstream scheme) matrix is fully supported. TLS is orthogonal on both sides, same as HttpProxy.

```
              upstream →  ws     wss
client ↓
ws                        ok     ok
wss                       ok     ok
```

Handshake sequence:

1. Extract `hyper::upgrade::on(&mut req)` from the client request _before_ destructuring. This is the one v1 idiom worth preserving.
2. Forward the upgrade request to the upstream over H1.1.
3. Extract `on(&mut upstream_req)` from the upstream's response.
4. Await both upgrades.
5. Hand off the bidirectional byte-tunnel to `Terminator::ByteTunnel`.

No WebSocket frame parsing in Vane. Post-upgrade, bytes are opaque.

### Close frame semantics — vane does not synthesize

Because vane is a byte tunnel after the upgrade, it does not synthesize or interpret `Close` control frames (opcode `0x8`). Consequences:

- If the client closes its TCP side (FIN), the upstream sees only a FIN — no preceding `Close` frame. RFC 6455 calls this an "abnormal closure" from the upstream's perspective, but it is **legal**; RFC 6455 §7.1.5 explicitly allows this case and applications are required to tolerate it.
- Conversely, a `Close` frame initiated by either endpoint flows through vane unchanged (it is just bytes in the tunnel). Vane does not acknowledge, translate, or delay it.
- Applications that depend on clean close handshakes (e.g., for flushing buffered messages) must handle the truncation case in their protocol layer — same as they would when speaking directly over a network that can drop connections.

This matches haproxy / envoy behavior in "tunnel" mode. Parsing frames to synthesize `Close` would re-introduce the frame-aware code path that v1 accumulated, which is exactly what `ByteTunnel`-by-design rejects.

### `L4Forward`

Byte-level duplex forward. TCP uses `tokio::io::copy_bidirectional` (which uses `splice(2)` on Linux when available); UDP uses a 5-tuple session-scoped forwarder with idle timeout.

For QUIC/HTTP-3 traffic, `udp_dispatch` demultiplexes before Fetch sees packets (see `06-l4.md`). `L4Forward { transport: Udp }` handles non-QUIC UDP.

### Variant ergonomics in config

Users write a `"type"` string in the rule's `terminate` block; the parser maps it to `(FetchKind, concrete Fetch impl)`. `FetchKind` is the core-side symbolic discriminant (see § _Concrete Fetch kinds_); the concrete impl is what `link` constructs in engine.

| JSON `type`      | FetchKind          | Concrete impl (engine)                                                                                                                                                         |
| ---------------- | ------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `tcp_forward`    | `L4Forward`        | `L4ForwardFetch { upstream, transport: Tcp, ... }`                                                                                                                             |
| `udp_forward`    | `L4Forward`        | `L4ForwardFetch { upstream, transport: Udp, ... }`                                                                                                                             |
| `http_proxy`     | `HttpProxy`        | `HttpProxyFetch { upstream: HttpUpstream::Tcp { version: Auto, .. }, ... }`                                                                                                    |
| `http1_proxy`    | `HttpProxy`        | `HttpProxyFetch { upstream: HttpUpstream::Tcp { version: Http1, .. }, ... }`                                                                                                   |
| `http2_proxy`    | `HttpProxy`        | `HttpProxyFetch { upstream: HttpUpstream::Tcp { version: Http2, .. }, ... }`                                                                                                   |
| `http3_proxy`    | `HttpProxy`        | `HttpProxyFetch { upstream: HttpUpstream::Tcp { version: Http3, .. }, ... }`                                                                                                   |
| `unix_proxy`     | `HttpProxy`        | `HttpProxyFetch { upstream: HttpUpstream::Unix { .. }, ... }`                                                                                                                  |
| `cgi`            | `HttpProxy`        | `HttpProxyFetch { upstream: HttpUpstream::Cgi { .. }, ... }`                                                                                                                   |
| `websocket`      | `WebSocketUpgrade` | `WebSocketUpgradeFetch { upstream, subprotocol, ... }`                                                                                                                         |
| `static`         | `HttpSynthesize`   | `HttpSynthesizeFetch { status, headers, body }`                                                                                                                                |
| `redirect_https` | `HttpSynthesize`   | `HttpSynthesizeFetch { status: 308, headers: { "location": "https://${host}${uri}" } }` — 308 preserves request method; dynamic Location built from the request's Host and URI |

Aliases are sugar; adding a new alias is a parser change, not a new `FetchKind` variant.

### Retry

Retry lives inside Fetch. A rule opting in configures:

- `max_attempts` — total attempts including the first. Default: `1` (no retry).
- `methods` — idempotent-method whitelist (GET / HEAD / PUT / DELETE / OPTIONS by default); POST / PATCH require explicit opt-in.
- `on` — `ErrorKind` set that triggers retry. Default: `{ Upstream, Timeout }`. Connection-pool failures always retry regardless.
- `backoff` — `none`, `fixed(Duration)`, or `exponential(base, max, jitter)`. Default: exponential with jitter.

Retry **implicitly forces request-body eager-buffering** (see `03-types.md` and `04-middleware.md`). `Body::Http12` and `Body::Http3` cannot be replayed; retry cannot proceed without buffering. Enabling retry is a deliberate memory-for-reliability tradeoff.

Retry is scoped to the single `HttpUpstream` configured for this Fetch. Multi-upstream failover is not a Fetch concern — express it via multiple rules with fallback predicates, each with its own Fetch.

---

## Terminator

Terminator is the final node of every path. It consumes whatever the preceding Fetch (and optional response-middleware chain) produced, writes it to the client socket, and closes.

Contract:

- **`WriteHttpResponse`** consumes a `Response`, serializes it over the client-side HTTP version (H1/H2/H3), then either closes or keeps the connection alive (H1 keep-alive / H2 / H3 multiplexing).
- **`ByteTunnel`** awaits the `Tunnel` established by `WebSocketUpgrade` or `L4Forward`. It neither drives the tunnel nor modifies bytes; it awaits completion and cleans up.

### Variants

```rust
pub enum Terminator {
    WriteHttpResponse,
    ByteTunnel,
}
```

Two variants is the complete set. The compiler enforces phase consistency (see `02-flow.md`):

- Paths through `HttpProxyFetch` or `HttpSynthesizeFetch` end in `WriteHttpResponse`.
- Paths through `WebSocketUpgradeFetch` or `L4ForwardFetch` end in `ByteTunnel`.

---

## Failure modes

Fetch and Terminator failures propagate as structured errors.

### Fetch failures

| Kind                               | HTTP response (L7)        | L4 action    |
| ---------------------------------- | ------------------------- | ------------ |
| Upstream unreachable               | `502 Bad Gateway`         | close client |
| Upstream timeout                   | `504 Gateway Timeout`     | close client |
| Upstream pool exhausted            | `503 Service Unavailable` | close client |
| Malformed upstream response        | `502 Bad Gateway`         | close client |
| WASM response-phase pool exhausted | `503 Service Unavailable` | —            |

Fetch failures do not retry inside the FlowGraph. Retry lives inside the Fetch itself — see the Retry subsection above and `07-l7.md` for the full retry policy.

### Terminator failures

Terminator failures are rare; by the time Terminator runs, the response has already been determined.

| Kind                         | Action                                               |
| ---------------------------- | ---------------------------------------------------- |
| Client socket write error    | Log, close connection, no further action             |
| Tunnel closed by either side | Normal termination, flush the other side, close both |

Terminator never produces a 5xx — a 5xx comes from Fetch or from a response-phase middleware rewriting the response.

---

## Why Fetch and Terminator are permanently built-in

- Fetch holds upstream socket handles. A plugin with socket access is a sandbox pierce.
- Terminator writes to the client socket and decides the connection's fate. A plugin with this capability can drop requests silently, inject responses, or leak data.
- Every prior "extensible terminator" design diverged on error semantics (each plugin handles "upstream down" differently) and duplicated pool/TLS logic per plugin.

If a new upstream mechanism is ever needed, it is added as a new `FetchInst` variant in `vaned` proper — a source-code change with review and testing, not a plugin drop-in.
