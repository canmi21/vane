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

| Crate                       | Tagline                                                                                      |
| --------------------------- | -------------------------------------------------------------------------------------------- |
| `acme-provider`             | `DnsProvider` trait for ACME DNS-01, with provider implementations behind feature flags.     |
| `clienthello`               | Extract the TLS SNI from QUIC Initial datagrams without performing a handshake.              |
| `guess`                     | Classify a TCP stream's first bytes as TLS / HTTP/2 / HTTP/1 / unknown.                      |
| `h3-body`                   | Adapt h3 server / client `RequestStream` to a single `http_body::Body` surface.              |
| `hyper-cgi`                 | Async CGI helpers: parse RFC 3875 stdout, stream the body, recognise reserved env keys.      |
| `notify-twophase`           | Two-phase notify-debouncer-full setup so events landing during server bind are not lost.     |
| `ocsp-mock-responder`       | In-process mock OCSP responder for integration tests.                                        |
| `ocsp-staple`               | Build OCSP requests, parse responses, and extract responder URLs from the AIA extension.     |
| `peeked-stream`             | Replay a peeked byte buffer back onto the read side of an `AsyncRead` + `AsyncWrite` stream. |
| `quinn-shared-socket`       | Run a `quinn::Endpoint` on a UDP socket shared with other consumers.                         |
| `rustls-crl-refresh`        | Process-wide CRL cache and refreshable rustls verifiers without `ServerConfig` churn.        |
| `rustls-native-roots-cache` | Process-wide cache for rustls's native trust store, with platform-aware retry.               |
| `rustls-ticketer`           | Install a process-wide rustls session ticketer once; idempotent across multiple call sites.  |
| `tokio-bind-retry`          | Bind a tokio `TcpListener` / `UdpSocket` with exponential backoff and cancellation support.  |
| `tracing-broadcast`         | `tracing_subscriber::Layer` that fans every event into a tokio broadcast channel as JSON.    |
| `virtual-socket`            | Demultiplex a single tokio `UdpSocket` into multiple virtual UDP sockets.                    |
