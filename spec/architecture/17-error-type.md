# Error Type

The single crate-level `Error` type for all of vane, plus the dispatch rules that map it to HTTP status, retry decisions, and observability channels. Implemented in `vane-core`; every other crate returns `Result<T, Error>`.

## Philosophy

Three layers of error handling are kept strictly separate:

1. **Typed propagation** (`vane-core::Error` via `thiserror`) — internal functions return typed errors with rich kind + reason + source chain. No stringly-typed error handling; no `anyhow` in library code.
2. **Structured tracing** (`tracing`) — every error production point emits a structured `tracing::event!` with `kind` / `reason` / context fields. Flow log and structured log both consume these as machine-filterable data.
3. **Terminal-pretty display** (`anyhow`) — only `vaned::main()` and `vane::main()` wrap the internal typed error into `anyhow::Error` so stderr (and, under systemd, journald) gets a legible multi-line error chain. Library code never imports `anyhow`.

This separation means:

- Retry decisions and HTTP status mapping read typed fields (`kind`, `reason`) — no regex on error strings.
- Metrics labels are bounded cardinality (`kind` has 9 variants, `reason` has ~20).
- TUI can reconstruct the full error chain for any connection through the management API (`tail_flow_log`) with structured JSON frames — not raw log scraping.

## `Error` struct

```rust
#[derive(thiserror::Error, Debug)]
#[error("{kind}{}", .ctx.as_deref().map(|c| format!(": {c}")).unwrap_or_default())]
pub struct Error {
    pub kind: ErrorKind,
    pub ctx:  Option<Cow<'static, str>>,
    #[source]
    pub source: Option<Box<dyn std::error::Error + Send + Sync>>,
}
```

Trait bounds:

- `Error: Send + Sync + 'static` — required for crossing await points and storing in `Arc<ConnContext>`.
- `Error: std::error::Error` — `thiserror` derives this; the `source()` method walks into the boxed cause.
- `Error: fmt::Debug + fmt::Display` — both derived; `Display` uses the `#[error(...)]` template.
- `Error` does **not** implement `Clone` (the boxed `source` has no general `Clone`) or `PartialEq` (semantic causal equality doesn't exist).

## `ErrorKind` with nested reason

Top-level kinds are kept flat and stable (9 variants) for low-cardinality metric labels. Fine-grained distinctions that matter for retry and HTTP status live on the associated `Reason` enums.

```rust
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    #[error("i/o")]                          Io,
    #[error("protocol")]                     Protocol,
    #[error("upstream: {0}")]                Upstream(UpstreamReason),
    #[error("middleware")]                   Middleware,
    #[error("compile")]                      Compile,
    #[error("timeout: {0}")]                 Timeout(TimeoutKind),
    #[error("canceled")]                     Canceled,
    #[error("resource: {0}")]                Resource(ResourceKind),
    #[error("internal")]                     Internal,
}

#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum UpstreamReason {
    #[error("unreachable")]           Unreachable,           // connect refused or unrouteable
    #[error("reset mid-request")]     ResetMidRequest,       // peer closed mid-send
    #[error("reset on idle pickup")]  ResetOnIdlePickup,     // stale conn from pool
    #[error("tls handshake failed")]  TlsHandshake,          // cert / config error
    #[error("dns resolution failed")] DnsFailure,            // resolver error
    #[error("refused by upstream")]   Refused,               // H2 RST_STREAM REFUSED_STREAM
    #[error("malformed response")]    Malformed,             // upstream spoke gibberish
}

#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum TimeoutKind {
    #[error("connect")]  Connect,
    #[error("read")]     Read,
    #[error("total")]    Total,
    #[error("idle")]     Idle,
    #[error("handshake")] Handshake,
}

#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum ResourceKind {
    #[error("connection pool exhausted")]  ConnectionPool,
    #[error("wasm pool exhausted")]        WasmPool,
    #[error("memory budget exceeded")]     Memory,
    #[error("file descriptors exhausted")] FdExhausted,
}
```

## Constructors

```rust
impl Error {
    pub fn new(kind: ErrorKind) -> Self { /* ... */ }

    pub fn with_ctx(mut self, ctx: impl Into<Cow<'static, str>>) -> Self { /* ... */ }
    pub fn with_source<E: Into<Box<dyn std::error::Error + Send + Sync>>>(mut self, e: E) -> Self { /* ... */ }

    // Convenience constructors for each top-level kind. Most production sites
    // use these rather than `Error::new(ErrorKind::...)`.
    pub fn io(msg: impl Into<Cow<'static, str>>) -> Self;
    pub fn protocol(msg: impl Into<Cow<'static, str>>) -> Self;
    pub fn upstream(reason: UpstreamReason) -> Self;
    pub fn middleware(msg: impl Into<Cow<'static, str>>) -> Self;
    pub fn compile(msg: impl Into<Cow<'static, str>>) -> Self;
    pub fn timeout(kind: TimeoutKind) -> Self;
    pub fn canceled() -> Self;
    pub fn resource(kind: ResourceKind) -> Self;
    pub fn internal(msg: impl Into<Cow<'static, str>>) -> Self;
}
```

## `From<>` impls (external-crate bridge)

Typed propagation depends on auto-converting well-known external errors into `vane_core::Error` via `?`. Each impl attaches the original as `source` so the chain is preserved:

```rust
impl From<std::io::Error> for Error { /* → ErrorKind::Io, source = e */ }
impl From<hyper::Error> for Error { /* → classified into Upstream/Protocol by variant */ }
impl From<h3::Error> for Error { /* → Upstream or Protocol */ }
impl From<rustls::Error> for Error { /* → Upstream(TlsHandshake) when client-side, Protocol when server-side */ }
impl From<fancy_regex::Error> for Error { /* → Compile */ }
impl From<serde_json::Error> for Error { /* → Compile */ }
impl From<ipnet::AddrParseError> for Error { /* → Compile */ }
impl From<hickory_resolver::ResolveError> for Error { /* → Upstream(DnsFailure) */ }
impl From<tokio::time::error::Elapsed> for Error { /* → Timeout — callers pick the exact TimeoutKind via with_ctx */ }
```

Impls live in `vane-core` behind `#[cfg(feature = "...")]` where the external crate is itself optional in vane-core's deps — for MVP they're all unconditional since vane-core pulls those crates per `16-crate-layout.md`.

## Query methods

```rust
impl Error {
    /// Top-level kind label — stable, low-cardinality. Suitable for metric labels.
    pub fn kind_label(&self) -> &'static str {
        match &self.kind {
            ErrorKind::Io          => "io",
            ErrorKind::Protocol    => "protocol",
            ErrorKind::Upstream(_) => "upstream",
            ErrorKind::Middleware  => "middleware",
            ErrorKind::Compile     => "compile",
            ErrorKind::Timeout(_)  => "timeout",
            ErrorKind::Canceled    => "canceled",
            ErrorKind::Resource(_) => "resource",
            ErrorKind::Internal    => "internal",
        }
    }

    /// Fine-grained reason label. Some kinds have no reason; they return None.
    pub fn reason_label(&self) -> Option<&'static str> {
        match &self.kind {
            ErrorKind::Upstream(r) => Some(match r {
                UpstreamReason::Unreachable       => "unreachable",
                UpstreamReason::ResetMidRequest   => "reset_mid_request",
                UpstreamReason::ResetOnIdlePickup => "reset_idle_pickup",
                UpstreamReason::TlsHandshake      => "tls_handshake",
                UpstreamReason::DnsFailure        => "dns_failure",
                UpstreamReason::Refused           => "refused",
                UpstreamReason::Malformed         => "malformed",
            }),
            ErrorKind::Timeout(t) => Some(match t {
                TimeoutKind::Connect   => "connect",
                TimeoutKind::Read      => "read",
                TimeoutKind::Total     => "total",
                TimeoutKind::Idle      => "idle",
                TimeoutKind::Handshake => "handshake",
            }),
            ErrorKind::Resource(r) => Some(match r {
                ResourceKind::ConnectionPool => "connection_pool",
                ResourceKind::WasmPool       => "wasm_pool",
                ResourceKind::Memory         => "memory",
                ResourceKind::FdExhausted    => "fd_exhausted",
            }),
            _ => None,
        }
    }

    /// Retry decision — consumed by Fetch's retry loop (see 07-l7.md retry policy).
    pub fn is_retryable(&self) -> bool {
        match &self.kind {
            ErrorKind::Upstream(r) => matches!(
                r,
                UpstreamReason::Unreachable
                    | UpstreamReason::ResetOnIdlePickup
                    | UpstreamReason::DnsFailure
                    | UpstreamReason::Refused,
            ),
            ErrorKind::Timeout(TimeoutKind::Connect) => true,
            ErrorKind::Resource(ResourceKind::ConnectionPool) => true,  // transient pool pressure
            _ => false,
        }
    }

    /// HTTP status code for the response, when this Error is returned on an L7 path.
    /// See 05-terminator.md and 04-middleware.md for invocation points.
    pub fn http_status(&self) -> u16 {
        match &self.kind {
            ErrorKind::Protocol        => 400,
            ErrorKind::Upstream(_)     => 502,
            ErrorKind::Timeout(_)      => 504,
            ErrorKind::Resource(_)     => 503,
            ErrorKind::Middleware      => 500,
            ErrorKind::Canceled        => 499,   // nginx convention: client abandoned
            ErrorKind::Compile         => 500,   // should never reach the HTTP layer
            ErrorKind::Internal        => 500,
            ErrorKind::Io              => 500,
        }
    }

    /// Walk `source()` to the root and collect each step's Display string.
    /// Consumed by the flow log event encoder to populate `source_chain`.
    pub fn source_chain(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur: &dyn std::error::Error = self;
        while let Some(src) = cur.source() {
            out.push(src.to_string());
            cur = src;
        }
        out
    }
}
```

## Three-layer dispatch in practice

### Layer A — `vane-core` typed propagation

Every internal function returns `Result<T, vane_core::Error>`. Use `?` and `From<>` conversions freely:

```rust
async fn fetch_upstream(req: Request) -> Result<Response, Error> {
    let conn = pool.get().await?;                       // hyper::Error → Upstream
    let resp = conn.send(req).await?;
    Ok(resp)
}
```

### Layer B — structured tracing at production sites

Every place that produces or wraps an Error also emits a structured event:

```rust
match fetch_upstream(req).await {
    Ok(r)  => Ok(r),
    Err(e) => {
        tracing::warn!(
            kind     = e.kind_label(),
            reason   = e.reason_label().unwrap_or(""),
            conn_id  = %ctx.conn.id(),
            upstream = %upstream_addr,
            "fetch failed",
        );
        Err(e)
    }
}
```

The `kind` / `reason` fields are structured attributes — downstream consumers (flow log sink, metrics exporter, management API streaming) read them as keys, not by parsing the message string.

### Layer C — `anyhow` only at binary entry

`vaned::main()` and `vane::main()` return `anyhow::Result<()>`. The typed `Error` auto-converts via `?` because it implements `std::error::Error + Send + Sync + 'static`:

```rust
// crates/daemon/src/main.rs
fn main() -> anyhow::Result<()> {
    dotenvy::from_path(env_path).ok();
    init_tracing()?;
    vane_engine::crypto::install_default_provider()?;
    run_daemon().map_err(anyhow::Error::from)?;
    Ok(())
}
```

`anyhow` prints the full error chain on stderr with nice formatting:

```
Error: upstream: tls handshake failed: verifying cert for api.example.com

Caused by:
    0: rustls: InvalidCertificate(UnknownIssuer)
    1: webpki: UnknownIssuer
```

`anyhow` appears **only** in the two binary `main.rs` files and nowhere else.

## Flow log error events

When an error reaches the flow log sink, it's serialized as:

```jsonc
{
	"t": 1234567890123,
	"conn": "abc123",
	"seq": 7,
	"event": "error",
	"node": 42, // FlowGraph node id where the error surfaced
	"error": {
		"kind": "upstream",
		"reason": "tls_handshake",
		"message": "upstream: tls handshake failed",
		"ctx": "verifying cert for api.example.com",
		"source_chain": ["rustls: InvalidCertificate(UnknownIssuer)", "webpki: UnknownIssuer"],
		"http_status": 502,
		"retryable": false
	}
}
```

TUI (`vane tail`) subscribes to `tail_flow_log` on the management API, filters by `conn` field, and renders the full error chain for the selected connection — no log scraping, no regex.

## Metrics

```
vane.errors_total{crate, kind, reason}             counter
    crate  = "engine" | "wasm" | "mgmt" | ...
    kind   = result of `kind_label()`
    reason = result of `reason_label()` or "none"
```

`reason = "none"` is emitted (rather than dropping the label) so Prometheus series cardinality is predictable. Total label cardinality: `crates × 9 kinds × 20 reasons ≈ 1000` — well within operational bounds.

## Observability conventions summary

| Channel         | Field source                                        | Purpose                               |
| --------------- | --------------------------------------------------- | ------------------------------------- |
| tracing event   | `kind_label`, `reason_label`                        | structured log for operators          |
| metrics counter | `kind_label`, `reason_label`                        | long-term trend + alerting            |
| flow log event  | full Error + source_chain + http_status + retryable | per-connection debug and audit        |
| HTTP response   | `http_status()` + generic body                      | client-facing signal                  |
| anyhow terminal | `Debug + Display + source chain`                    | pretty stderr only in binary `main()` |
