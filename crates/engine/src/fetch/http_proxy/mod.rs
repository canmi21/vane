//! `HttpProxyFetch` — pooled, ALPN-aware reverse-proxy fetch.
//!
//! Forwards the decoded `Request` to a configured upstream HTTP
//! server and returns its `Response` to the executor. Two dispatch
//! paths:
//!
//! * **TCP family (H1 / H2)** — owned by
//!   `hyper_util::client::legacy::Client` over a
//!   `hyper_rustls::HttpsConnector<HttpConnector>`: per-authority
//!   connection pooling, ALPN-driven H1 / H2 negotiation on TLS,
//!   cleartext h2c via prior knowledge when the rule pins
//!   `version: "h2"` without TLS.
//! * **QUIC family (H3)** — owned by [`super::quic_pool`]: per-fingerprint
//!   pooled `quinn::Endpoint` + `h3::client` send-request handle, mandatory
//!   TLS with ALPN `[b"h3"]`. Compiled in only when the `h3` cargo
//!   feature is on; without it, `version: "h3"` is rejected at
//!   factory time.
//!
//! The `version` field selects the upstream's HTTP version posture.
//! Permitted values mirror `spec/crates/core.md` § _Compile pipeline_ (`version` row):
//!
//! | `version` | TLS upstream                | Cleartext upstream     |
//! | --------- | --------------------------- | ---------------------- |
//! | `auto`    | ALPN: prefer `h2`, fall H1  | H1 (no ALPN; warn)     |
//! | `h1`      | ALPN: only `http/1.1`       | H1                     |
//! | `h2`      | ALPN: only `h2`             | h2c (prior knowledge)  |
//! | `h3`      | ALPN: only `h3` (TLS req'd) | rejected (h3 mandates QUIC TLS) |
//!
//! See `spec/crates/engine.md` `spec/crates/engine.md` § _Concrete fetches_,
//! `spec/crates/engine.md` § _Body streaming_, § _Upstream pools_,
//! and `spec/crates/engine-tls.md` § _Library policy_.
//!
//! ## Module layout
//!
//! Sub-files cut between "runtime dispatch" and "build-time factory":
//!
//! - [`dispatch`] — `impl L7Fetch for HttpProxyFetch` and the
//!   per-request send / receive helpers (TCP retry loop + H3
//!   send-body + recv-response pump).
//! - [`factory`] — args parsing, `build_client`, `build_h3_dispatch`,
//!   the public `factory` / `register` entry points, and the factory
//!   test suite. Public re-exports flow back through this module so
//!   downstream callers continue to import
//!   `crate::fetch::http_proxy::{factory, register}`.

use std::sync::Arc;

use crate::fetch::client_cache::ProxyClient;
use crate::fetch::retry::RetryPolicy;

mod dispatch;
mod factory;

pub use factory::{factory, register};

/// Upstream HTTP-version posture. Pinned at factory time from
/// `args.version`. `Http3` is gated behind the `h3` cargo feature;
/// builds without it reject `version: "h3"` at factory time.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum UpstreamVersion {
	Auto,
	Http1,
	Http2,
	#[cfg(feature = "h3")]
	Http3,
}

/// Reverse-proxy fetch dispatched through one of two pool families:
/// `Tcp` (via [`crate::fetch::client_cache`] for the H1 / H2 path)
/// or `Quic` (via [`crate::fetch::quic_pool`] for the H3 path). Each
/// family owns its own pooling discipline; see the module-level
/// docstring for the full posture matrix.
pub struct HttpProxyFetch {
	pub(super) version: UpstreamVersion,
	/// `http::uri::Scheme` resolved once at factory time. Both
	/// variants ([`http::uri::Scheme::HTTP`] / [`http::uri::Scheme::HTTPS`])
	/// are cheap to clone (refcount under the hood) — per-request
	/// `format!`-driven URI rebuild is avoided in favour of
	/// `Uri::builder().scheme(...).authority(...)`.
	pub(super) scheme: http::uri::Scheme,
	/// `http::uri::Authority` parsed once at factory time and
	/// `Clone`d per request (`Authority` wraps a refcounted `Bytes`).
	pub(super) authority: http::uri::Authority,
	pub(super) dispatch: Dispatch,
	pub(super) retry: Arc<RetryPolicy>,
}

/// Per-version dispatch state. `Tcp` carries the cached pooled
/// `legacy::Client`; `Quic` carries the rustls config the QUIC pool
/// needs at dial time plus the SNI / TLS-fingerprint pieces the
/// per-request fingerprint composes from.
pub(super) enum Dispatch {
	Tcp(Arc<ProxyClient>),
	#[cfg(feature = "h3")]
	Quic(QuicDispatchState),
}

/// State the H3 dispatch arm carries at factory time. `addr` is
/// resolved per-request (from `upstream`) so DNS changes affect new
/// pool entries; the existing entries keyed under the prior `addr`
/// stay live until quinn's idle timeout retires them. `connect_timeout`
/// caps the H3 dial half (DNS + UDP bind + QUIC handshake + h3
/// negotiation); pool-hit fast paths short-circuit before reaching
/// it. Pool entries retired by quinn's own idle timeout are lazily
/// re-dialed against this same ceiling on the next miss.
#[cfg(feature = "h3")]
pub(super) struct QuicDispatchState {
	pub(super) rustls_cfg: Arc<rustls::ClientConfig>,
	pub(super) sni: Arc<str>,
	pub(super) tls_fp: crate::fetch::client_cache::TlsConfigFingerprint,
	pub(super) connect_timeout: std::time::Duration,
	/// Per-fetch hickory resolver. Built once at factory time from the
	/// rule's `args.dns`; every dial composes its `IpAddr` result with
	/// `port` to feed `quinn::Endpoint::connect`. Sharing the resolver
	/// across dials lets hickory's TTL cache absorb repeat lookups.
	pub(super) resolver: Arc<crate::fetch::dns::HickoryDnsResolver>,
	/// Pre-parsed host portion of `args.upstream`. Bracketed IPv6
	/// literals (`[::1]`) are stripped here so the resolver short-
	/// circuit reaches `IpAddr::parse` cleanly.
	pub(super) host: Arc<str>,
	/// Pre-parsed port portion of `args.upstream`. The dial composes
	/// this with the resolved IP into a `SocketAddr`.
	pub(super) port: u16,
}

/// Spec default for the H3 dial half's `connect_timeout` when
/// `args.connect_timeout` is absent.
/// `spec/crates/engine.md` § _CGI_.
#[cfg(feature = "h3")]
pub(super) const H3_CONNECT_TIMEOUT_DEFAULT: std::time::Duration =
	std::time::Duration::from_secs(5);
