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

## Body lifecycle

Two bodies exist per L7 connection flow, owned and transferred in sequence:

1. **Request body** — created at L4→L7 upgrade; owned by `Request`; accessible as `&mut` to `L7RequestMiddleware`; consumed by Fetch (ownership moves into the upstream client or into synthesis).
2. **Response body** — produced by Fetch (from upstream or synthesis); owned by `Response`; accessible as `&mut` to `L7ResponseMiddleware`; consumed by `Terminator::WriteHttpResponse` (ownership moves into the client-side encoder).

Within a middleware's `&mut` borrow, the `Body` is a swappable value — `*req.body_mut() = Body::Static(new_bytes)` is the body-replacement idiom.

### Buffering and size

Buffering is **eager and compile-time-decided** (see `02-flow.md` and `04-middleware.md`). On paths where any reachable middleware declares `needs_body`, or where Fetch has retry enabled, the body is fully collected into `Body::Static(Bytes)` before any middleware runs.

`max_body_size` is per-rule (default **8 MiB**). Request body exceeding the limit during eager collection produces `413 Payload Too Large`. Response body exceeding it produces `502 Bad Gateway` (upstream violated the expected contract).

### Cancellation

Cancellation is **ownership-based**. When a client disconnects mid-stream, the hyper/h3 server task holding the request is dropped; `Drop` cascades through every `Arc` and `Future`. Fetch's `Future` is dropped, which signals upstream to close (RST_STREAM on H2/H3, connection close on H1). No explicit cancel token.

### Trailers

HTTP trailers (used by gRPC-over-H2) are transparent. `http_body::Body::poll_frame` yields `Frame<Bytes>` with two variants: `Frame::data` and `Frame::trailers`. The `Body` variants `Http12` and `Http3` pass through `Frame::trailers` verbatim. Ingress parsers produce them when the wire format contains trailers; egress encoders emit them as the target protocol's trailer form (H1 chunked trailers, H2/H3 trailer frames).

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
