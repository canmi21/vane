# Library

Generic, project-agnostic Rust crates developed alongside vane
but not bound to its domain. Every crate here is publishable to
crates.io as-is.

## What belongs here

- No `vane-` prefix; named after what the crate does.
- Public API uses generic terminology — no vane-internal names.
- No `vane-*` in `[dependencies]`; tests stay self-contained.
- MIT, matching the workspace.
- Standalone self-managed version.
- MSRV tracks the workspace `rust-toolchain.toml`.

## Current crates

| Crate                       | Tagline                                                                                       |
| --------------------------- | --------------------------------------------------------------------------------------------- |
| `acme-provider`             | `DnsProvider` trait for ACME DNS-01, with provider implementations behind feature flags.      |
| `cgi-request`               | Build the RFC 3875 environment-variable list for a CGI child from an HTTP request.            |
| `cgi-response`              | Parse a CGI child's stdout into an `http::Response` with a streaming body.                    |
| `clienthello`               | Extract the TLS SNI from QUIC Initial datagrams without performing a handshake.               |
| `guess`                     | Classify a TCP stream's first bytes as TLS / HTTP/2 / HTTP/1 / unknown.                       |
| `h3-body`                   | Adapt h3 server / client `RequestStream` to a single `http_body::Body` surface.               |
| `hickory-tower-resolver`    | Wrap `hickory-resolver` as a `tower::Service` for hyper-util's `HttpConnector`.               |
| `http-retry-policy`         | HTTP retry policy with exponential backoff, idempotent-method gating, and body-buffering.     |
| `ndjson-rpc`                | Line-delimited JSON-RPC over Unix sockets and HTTP/1.1 chunked, one-shot + streaming verbs.   |
| `notify-twophase`           | Two-phase notify-debouncer-full setup so events landing during server bind are not lost.      |
| `ocsp-mock-responder`       | In-process mock OCSP responder for integration tests.                                         |
| `ocsp-staple`               | Build OCSP requests, parse responses, and extract responder URLs from the AIA extension.      |
| `peeked-stream`             | Replay a peeked byte buffer back onto the read side of an `AsyncRead` + `AsyncWrite` stream.  |
| `prom-cardinality-cap`      | Per-namespace cap on Prometheus metric label cardinality with warn-once-on-first-drop.        |
| `quinn-shared-socket`       | Run a `quinn::Endpoint` on a UDP socket shared with other consumers.                          |
| `rustls-crl-refresh`        | Process-wide CRL cache and refreshable rustls verifiers without `ServerConfig` churn.         |
| `rustls-native-roots-cache` | Process-wide cache for rustls's native trust store, with platform-aware retry.                |
| `rustls-pem-roots`          | Load PEM-encoded CA certificates from files and directories into a rustls `RootCertStore`.    |
| `rustls-sni-resolver`       | SNI-keyed cert map implementing rustls's `ResolvesServerCert`, designed for `ArcSwap` reload. |
| `rustls-ticketer`           | Install a process-wide rustls session ticketer once; idempotent across multiple call sites.   |
| `tokio-bind-retry`          | Bind a tokio `TcpListener` / `UdpSocket` with exponential backoff and cancellation support.   |
| `tracing-broadcast`         | `tracing_subscriber::Layer` that fans every event into a tokio broadcast channel as JSON.     |
| `virtual-socket`            | Demultiplex a single tokio `UdpSocket` into multiple virtual UDP sockets.                     |
