# Core Types

## Principles

1. **Do not invent types when the ecosystem has standard ones.** `http` / `http-body` / `hyper` / `h3` / `rustls` converged years ago; diverging costs compatibility and buys nothing.
2. **Typed Extensions everywhere.** No string-keyed KV. No `dyn Any` downcasts. `http::Extensions` (keyed by `TypeId`) is the only escape hatch for protocol-specific fields.
3. **Arc-shared connection state.** Per-request state lives in `http::Request::extensions()`. Per-connection state lives behind `Arc<ConnContext>` and is inserted into every request's extensions on that connection.

## L7: `Request` and `Response`

```rust
pub type Request  = http::Request<Body>;
pub type Response = http::Response<Body>;
```

These are `http` crate aliases. Hyper produces and consumes them directly on the H1/H2 ingress path; the H3 path reconstructs them from `h3::server::RequestStream`.

## L7 body: `Body`

```rust
pub enum Body {
    // H1 / H2 server ingress via hyper
    Http12(hyper::body::Incoming),
    // H3 server ingress via the h3 crate
    Http3(H3Body),
    // Materialized bytes (middleware-produced, fixtures, fixed responses)
    Static(bytes::Bytes),
    // Arbitrary stream (WASM plugin output, custom producers)
    Stream(Pin<Box<dyn http_body::Body<Data = bytes::Bytes, Error = Error> + Send + 'static>>),
    // No body (HEAD, GET without body, 204, 304)
    Empty,
}

impl http_body::Body for Body {
    type Data = bytes::Bytes;
    type Error = Error;
    fn poll_frame(...) -> Poll<Option<Result<Frame<Bytes>, Error>>>;
    fn size_hint(&self) -> http_body::SizeHint;
    fn is_end_stream(&self) -> bool;
}
```

All variants implement `http_body::Body<Data = Bytes>`. The enum avoids vtable dispatch on the common path; the `Stream` variant absorbs extension cases where vtable cost is unavoidable.

Variant names are **protocol-named, not vendor-named** (per `spec/naming.md` — brand names only in edge modules). Type parameters reference upstream crates as edge types, but the variant name describes the protocol role.

### `H3Body`

`h3` crate does **not** implement `http_body::Body` — it splits the two concerns: `recv_data()` yields `impl Buf` (in current versions: `Bytes` slices over quinn's internal buffer) and `recv_trailers()` is a separate, once-only call at data EOF. `H3Body` is vane's glue that wraps both into a single `http_body::Body` stream:

```rust
pub struct H3Body<S> {
    stream: S,                         // h3::server::RequestStream or h3::client::RequestStream
    state:  H3BodyState,
}

enum H3BodyState {
    DataPhase,                         // recv_data() yields Frame::data(Bytes)
    TrailerPhase,                      // recv_trailers() yields Frame::trailers(HeaderMap) or None
    Done,
}

impl<S> http_body::Body for H3Body<S> where S: /* h3 stream trait */ {
    type Data  = bytes::Bytes;
    type Error = Error;

    // poll_frame dispatches on state:
    //   DataPhase    → recv_data()  → Frame::data(Bytes); at data EOF, transition to TrailerPhase
    //   TrailerPhase → recv_trailers() once → Frame::trailers(HeaderMap) or skip; then Done
    //   Done         → Poll::Ready(None)
}
```

Server and client use different `h3` stream types; `S` is monomorphized at each construction site. The `Bytes` returned by `recv_data()` slices quinn's buffer pool — no copy on the vane side.

### What vane's "no copy" covers (and what it does not)

The `Body` enum is a zero-copy pipe **at vane's layer**: `poll_frame` returns `Frame<Bytes>` whose payload was already a `Bytes` produced by the ingress parser (hyper for H1/H2, `H3Body` wrapping `h3::RequestStream::recv_data` for H3). Vane neither copies nor accumulates these `Bytes` before handing them to the upstream encoder. It follows that:

- **QUIC packet reassembly** (multi-datagram coalescing, out-of-order stream-offset handling, retransmission cache) happens inside `quinn` / `h3`; vane makes no zero-copy claim about it.
- **H2 flow-control window accounting** happens inside the `h2` crate; its buffering is that crate's concern.
- **H1 chunked decode and encode** happen inside `hyper`; the per-chunk size-prefix allocation on encode and the chunk-boundary `Bytes` production on decode are hyper's cost, not vane's.

Vane's guarantee is the absence of a **vane-introduced** copy — not that the whole stack from wire to upstream is allocation-free. Users who want to know "is this deployment truly zero-copy from kernel to origin?" must additionally audit quinn / h3 / hyper's own cost models.

### `BodyStreamAdapter`

Producers that implement `http_body::Body` with a foreign `Error` type (WASM plugin outputs, custom streaming sources, CGI buffered body wrappers) use a standard adapter to land as `Body::Stream`:

```rust
pub struct BodyStreamAdapter<B> {
    inner: B,
}

impl<B, E> http_body::Body for BodyStreamAdapter<B>
where
    B: http_body::Body<Data = bytes::Bytes, Error = E> + Send + 'static,
    E: Into<Error> + Send + Sync + 'static,
{
    type Data  = bytes::Bytes;
    type Error = Error;
    // poll_frame forwards to inner, mapping Err(E) → Err(E.into())
}

impl Body {
    pub fn from_producer<B, E>(producer: B) -> Self
    where
        B: http_body::Body<Data = bytes::Bytes, Error = E> + Send + 'static,
        E: Into<Error> + Send + Sync + 'static,
    {
        Self::Stream(Box::pin(BodyStreamAdapter { inner: producer }))
    }
}
```

The `E: Into<Error>` bound means every custom producer only needs to provide a `From<CustomError> for Error` impl (one line with `#[from]` on `ErrorKind`) to participate. WASM plugins' produced bodies plug through this adapter.

The `Box::pin` inside `from_producer` is a once-per-body allocation (paid at producer construction, not per frame). Per-frame `poll_frame` delegates to the inner producer with no additional copy — this is the `Body::Stream` path's steady-state cost.

### The `'static` bound on `Body::Stream`

`Body::Stream` is `dyn Body + Send + 'static`. This means a producer must not borrow from anything that outlives a single request — in practice it must own its data (most commonly via `Bytes`, which is already refcounted and cheap to clone from buffered state). Middleware that wants to "replace body with data it holds" takes the explicit path: materialize to owned `Bytes`, then `*req.body_mut() = Body::Static(bytes)` — no borrow plumbing, no lifetime parameters infecting the `Body` enum. Every production use-case (buffered rewrites, synthesized bodies, proxied streams from pooled upstream clients) fits this shape naturally; a lifetime-parameterized `Body<'a>` was considered and rejected for the cost of polluting every `Request`/`Response` signature downstream.

### `Body::as_static`

Post-buffering readers (the most common being `http.body` predicates) rely on a simple accessor:

```rust
impl Body {
    /// Returns `Some(&Bytes)` iff this body has already been collected to
    /// `Body::Static`. Returns `None` for stream-typed variants.
    pub fn as_static(&self) -> Option<&bytes::Bytes> {
        if let Self::Static(b) = self { Some(b) } else { None }
    }
}
```

By the phase machine + LazyBuffer compile-time analysis, any reader of `http.body` on a given path is guaranteed to run **after** the eager-collect point for that side, so `as_static().expect("lazy-buffer invariant")` is a legal pattern at the call site.

## Body lifecycle

Two bodies exist per L7 connection flow, owned and transferred in sequence:

1. **Request body** — created at L4→L7 upgrade; owned by `Request`; accessible as `&mut` to `L7RequestMiddleware`; consumed by `L7Fetch::fetch` (ownership moves into the upstream client or into synthesis).
2. **Response body** — produced by `L7Fetch` (from upstream or synthesis); owned by `Response`; accessible as `&mut` to `L7ResponseMiddleware`; consumed by `Terminator::WriteHttpResponse` (ownership moves into the client-side encoder).

Within a middleware's `&mut` borrow, the `Body` is a swappable value — `*req.body_mut() = Body::Static(new_bytes)` is the body-replacement idiom.

### Buffering is two-track and compile-time-decided

Buffering is **eager and compile-time-decided** (see `02-flow.md`'s LazyBuffer section), but **request-side and response-side are independent tracks**. A single rule can be request-buffered (e.g., a middleware validates the request body) and response-streaming (the response flows through untouched), or vice-versa, or both-buffered, or both-streaming.

A path is request-buffered iff **any** of the following is reachable from the path entry to the `L7Fetch`:

- a `L7RequestMiddleware` on that path declares `needs_body() == true`, or
- a `Check` node on that path reads the `http.body` field (request side), or
- the terminating `L7Fetch` has retry enabled.

A path is response-buffered iff **any** of the following is reachable from the `L7Fetch`'s `next_response` edge to the `WriteHttpResponse` terminator:

- a `L7ResponseMiddleware` on that path declares `needs_body() == true`, or
- (future) a `Check` node on that path reads a response-side body field.

The compiler attaches a `collect_body_before: Option<BodySide>` flag to the **first** node that requires buffered bytes on each side (see `02-flow.md`). The executor collects at that point and replaces the body with `Body::Static(Bytes)`; nodes downstream on that side observe buffered bytes.

Once collected, `Body::Static` is **replay-safe** (it is a `Bytes` — refcounted, cheap to clone). Fetch retry is enabled by this property: the retry loop keeps the `Body::Static` around and clones it into each attempt.

`max_body_size` is per-rule (default **8 MiB**). Request body exceeding the limit during eager collection produces `413 Payload Too Large`. Response body exceeding it produces `502 Bad Gateway` (upstream violated the expected contract). The two limits can be configured independently; omitting either defaults to the 8 MiB value.

### Cancellation

Cancellation is **ownership-based**. When a client disconnects mid-stream, the hyper/h3 server task holding the request is dropped; `Drop` cascades through every `Arc` and `Future`. Fetch's `Future` is dropped, which signals upstream to close (RST_STREAM on H2/H3, connection close on H1). No explicit cancel token.

### Trailers

HTTP trailers (used by gRPC-over-H2) are transparent. `http_body::Body::poll_frame` yields `Frame<Bytes>` with two variants: `Frame::data` and `Frame::trailers`. The `Body` variants `Http12` and `Http3` pass through `Frame::trailers` verbatim. Ingress parsers produce them when the wire format contains trailers; egress encoders emit them as the target protocol's trailer form (H1 chunked trailers, H2/H3 trailer frames).

**H1 egress-side framing decision**: H1 can carry trailers only via `Transfer-Encoding: chunked` (RFC 9112 §7.1.2), not via `Content-Length`. When the outgoing `Body` on an H1 egress is `Body::Http12(_)` (from an H2 upstream whose body may carry trailers) or `Body::Http3(_)`, **the H1 encoder unconditionally selects `Transfer-Encoding: chunked` and strips any `Content-Length` header**. When the outgoing `Body` is `Body::Static(_)`, the encoder uses `Content-Length` (exact size is known, chunked framing would only add overhead). `Body::Empty` writes no body and no content framing header. `Body::Stream` defaults to chunked. The encoder owns this decision; middleware never does.

## L4: `L4Conn`

```rust
pub enum L4Conn {
    Tcp(tokio::net::TcpStream),
    Udp(UdpAssoc),
}

pub struct UdpAssoc {
    socket: Arc<tokio::net::UdpSocket>,
    peer:   SocketAddr,
    quic:   Option<QuicAssocId>,  // set when this datagram belongs to an existing QUIC session
}
```

L4 connections never construct `http::Request`. L4 middleware operates on `L4Conn` directly.

## Per-connection context: `ConnContext`

```rust
pub struct ConnContext {
    pub remote:     std::net::SocketAddr,
    pub local:      std::net::SocketAddr,
    pub transport:  Transport,
    pub entered_at: std::time::Instant,

    // Populated progressively: the L4 peek phase writes sni/alpn from the
    // ClientHello; the L4→L7 upgrade phase writes version/peer_cert after
    // the full handshake. Mutex because writes are sequential across phase
    // transitions — contention is effectively zero but we need interior
    // mutability through Arc.
    pub tls:          parking_lot::Mutex<Option<TlsInfo>>,
    pub http_version: std::sync::OnceLock<HttpVersion>,

    // User-defined typed slots. Lock is cheap (parking_lot); contention is rare
    // because only middleware writes, and at most one middleware writes per hop.
    pub user: parking_lot::Mutex<http::Extensions>,
}

pub struct TlsInfo {
    pub sni:       Option<String>,          // from ClientHello (L4 peek) or handshake
    pub alpn:      Option<Vec<u8>>,         // from ClientHello / ALPN negotiation
    pub version:   Option<rustls::ProtocolVersion>,           // known after handshake
    pub peer_cert: Option<rustls::pki_types::CertificateDer<'static>>,  // after handshake, if mTLS
}

pub enum Transport { Tcp, Udp }
pub enum HttpVersion { Http1_0, Http1_1, Http2, Http3 }
```

Invariants:

- `remote`, `local`, `transport`, `entered_at` are set at accept and never mutate.
- `tls` uses `Mutex<Option<TlsInfo>>` to allow progressive population across phase transitions. Readers (predicates, middleware) observe `None` until the first write (peek) and a progressively-filled `Some(TlsInfo)` thereafter.
- `http_version` uses `OnceLock` — set exactly once during L4→L7 upgrade, read freely afterward.
- `user` is a typed anymap. Read is cheap; write is guarded by a lock that is essentially uncontended in practice.

Every `Request` on this connection carries `Arc<ConnContext>` in `request.extensions()`. H2 and H3 streams multiplexed on one connection share the same `Arc`. Refcount reaching zero releases the context.

## Execution context: `FlowCtx`

`ConnContext` is the connection-level, mostly-immutable shared state (one `Arc` per TCP/QUIC connection, read by all middleware on all multiplexed streams). `FlowCtx` is the complementary **per-execution, mutable** state — one `FlowCtx` per executor invocation, owned on the executor's stack and borrowed `&mut` to every middleware / Fetch call.

```rust
pub struct FlowCtx<'a> {
    pub graph:  &'a FlowGraph,
    pub span:   &'a mut tracing::Span,       // current flow-log span; middleware may enter children
    pub log:    &'a mut FlowLogSink,         // structured event sink for this execution
    pub cancel: &'a tokio_util::sync::CancellationToken,
}
```

Every async trait in `04-middleware.md` and `05-terminator.md` takes two context parameters:

- `conn: &Arc<ConnContext>` — connection-shared state (read, and `user`-extensions write)
- `ctx:  &mut FlowCtx<'_>` — execution-scoped state (span nesting, log emission, cancel observation)

The split makes the two axes explicit: **shared/unchanging vs. execution/mutable**. It also makes the `&mut` meaningful — previously a single `&mut Ctx` existed but none of its fields were actually mutable, which confused both trait authors and the executor.

The executor is the sole producer of `FlowCtx`. Middleware never constructs one; middleware receives the executor's `FlowCtx` by mutable borrow and is forbidden from leaking its fields beyond the `run` call.

## Connection lifecycle

```
accept()                     → construct Arc<ConnContext>
L4 middleware chain          → reads ConnContext; may write to user extensions
L4→L7 upgrade (optional)     → set ctx.tls, ctx.http_version
protocol decode              → emit Request with Arc<ConnContext> in extensions
L7 middleware chain          → reads Request and ConnContext
Terminator                   → forwards or responds
drop last Arc<ConnContext>   → OnceLocks and user extensions drop
```

No user-authored destructor. Refcount handles cleanup.

## Protocol-specific metadata

Protocol-specific fields (H2 stream priority, H3 flow control hints, HTTP trailers, WebSocket upgrade handle, peer TLS certificate detail, etc.) live in `request.extensions()` as typed entries:

```rust
pub struct H2Priority { pub weight: u8, pub depends_on: Option<u32> }
pub struct Trailers(pub http::HeaderMap);
pub struct WsUpgrade(pub hyper::upgrade::OnUpgrade);
```

Middleware that cares reads the type directly. Middleware that does not care ignores it. The map is keyed by `TypeId` — no strings, no downcasts.

## Error type

```rust
pub struct Error { /* opaque */ }

impl Error {
    pub fn kind(&self) -> ErrorKind;
    pub fn source(&self) -> Option<&(dyn std::error::Error + 'static)>;
}

pub enum ErrorKind {
    Io,          // socket / file I/O
    Protocol,    // HTTP parse, malformed frames, H2/H3 protocol violations
    Upstream,    // upstream network errors (connect, TLS, response format)
    Middleware,  // middleware returned Err (including WASM plugin-error)
    Compile,     // rule compilation / validation failed
    Timeout,     // deadline elapsed
    Canceled,    // request canceled (client disconnect, management command)
    Resource,    // capacity exhausted (pool, memory budget, FD, WASM pool)
    Internal,    // assertion that should never fire
}
```

A single crate-level error type with a typed `ErrorKind`. No per-crate error proliferation inside `vaned`. `anyhow` is reserved for `vane` CLI-level sites only. At the management API boundary, errors are serialized as `{ kind, message, source? }`.
