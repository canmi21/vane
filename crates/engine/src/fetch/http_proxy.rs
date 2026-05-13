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

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use vane_core::{
	Body, ConnContext, Error, FetchKind, FlowCtx, L7Fetch, L7FetchOutput, Request, UpstreamReason,
};

use crate::body_adapter::IncomingAdapter;
use crate::factories::{FactoryError, FetchFactories};
use crate::fetch::client_cache::{ClientFingerprint, ProxyClient};
use crate::fetch::dns::{DnsConfig, HickoryDnsResolver, parse_dns_args};
use crate::fetch::retry::RetryPolicy;
use crate::fetch::upstream::{UpstreamTls, parse_tls_args};
use crate::flow_graph::FetchInst;

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
	version: UpstreamVersion,
	/// `http::uri::Scheme` resolved once at factory time. Both
	/// variants ([`http::uri::Scheme::HTTP`] / [`http::uri::Scheme::HTTPS`])
	/// are cheap to clone (refcount under the hood) — per-request
	/// `format!`-driven URI rebuild is avoided in favour of
	/// `Uri::builder().scheme(...).authority(...)`.
	scheme: http::uri::Scheme,
	/// `http::uri::Authority` parsed once at factory time and
	/// `Clone`d per request (`Authority` wraps a refcounted `Bytes`).
	authority: http::uri::Authority,
	dispatch: Dispatch,
	retry: Arc<RetryPolicy>,
}

/// Per-version dispatch state. `Tcp` carries the cached pooled
/// `legacy::Client`; `Quic` carries the rustls config the QUIC pool
/// needs at dial time plus the SNI / TLS-fingerprint pieces the
/// per-request fingerprint composes from.
enum Dispatch {
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
struct QuicDispatchState {
	rustls_cfg: Arc<rustls::ClientConfig>,
	sni: Arc<str>,
	tls_fp: crate::fetch::client_cache::TlsConfigFingerprint,
	connect_timeout: std::time::Duration,
	/// Per-fetch hickory resolver. Built once at factory time from the
	/// rule's `args.dns`; every dial composes its `IpAddr` result with
	/// `port` to feed `quinn::Endpoint::connect`. Sharing the resolver
	/// across dials lets hickory's TTL cache absorb repeat lookups.
	resolver: Arc<HickoryDnsResolver>,
	/// Pre-parsed host portion of `args.upstream`. Bracketed IPv6
	/// literals (`[::1]`) are stripped here so the resolver short-
	/// circuit reaches `IpAddr::parse` cleanly.
	host: Arc<str>,
	/// Pre-parsed port portion of `args.upstream`. The dial composes
	/// this with the resolved IP into a `SocketAddr`.
	port: u16,
}

/// Spec default for the H3 dial half's `connect_timeout` when
/// `args.connect_timeout` is absent.
/// `spec/crates/engine.md` § _CGI_.
#[cfg(feature = "h3")]
const H3_CONNECT_TIMEOUT_DEFAULT: std::time::Duration = std::time::Duration::from_secs(5);

#[async_trait]
impl L7Fetch for HttpProxyFetch {
	async fn fetch(
		&self,
		mut req: Request,
		_conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<L7FetchOutput, Error> {
		// Compose the upstream URI from `scheme + authority` resolved
		// once at factory time and the inbound path/query, refcounted
		// where possible. For TCP (hyper-util Client) the connector
		// reads `http://` / `https://` to pick cleartext vs TLS; for QUIC
		// (h3 client) the scheme + authority become :scheme / :authority
		// pseudo-headers so the upstream sees the rewritten target.
		// `PathAndQuery` clones are refcounted; `Scheme` / `Authority`
		// clones are `Bytes`-backed refcount bumps. No format!, no
		// per-request parse — only the `Uri::builder` assembly cost.
		let path_and_query = req
			.uri()
			.path_and_query()
			.cloned()
			.unwrap_or_else(|| http::uri::PathAndQuery::from_static("/"));
		*req.uri_mut() = http::Uri::builder()
			.scheme(self.scheme.clone())
			.authority(self.authority.clone())
			.path_and_query(path_and_query)
			.build()
			.map_err(|e| Error::protocol("upstream uri rewrite").with_source(e))?;

		// Snapshot the request body for replay. `Body::Stream` is
		// one-shot — it collapses retry to a single attempt
		// regardless of `max_attempts`. `Body::Static` clones via
		// `Bytes` refcount; `Body::Empty` replays as zero-length
		// `Bytes`. Per `spec/crates/engine.md`
		// § _Retry_, this is the `opportunistic` rule:
		// streaming bodies skip retry quietly. `force` buffering is
		// implemented earlier in the lower pass — by the time the
		// fetch sees the request, a `force` policy has already
		// converted the body to `Body::Static`. The TCP and H3 arms
		// below share this snapshot — their retry semantics are
		// symmetric per `spec/crates/engine.md` § _Retry_.
		//
		// `method_allowed` is the operator-configured whitelist
		// (defaults to the RFC 9110 idempotent set). The per-attempt
		// check below additionally consults
		// `Error::is_retryable_in(method)` so a `ResetMidRequest`
		// against a whitelisted-but-non-idempotent method (e.g. a
		// custom config that whitelists POST) still bails after one
		// attempt — body double-delivery isn't safe even with the
		// whitelist's opt-in.
		let method_allowed = self.retry.methods.contains(req.method());
		let replay: Option<Bytes> = match req.body() {
			Body::Static(b) => Some(b.clone()),
			Body::Empty => Some(Bytes::new()),
			Body::Stream(_) => None,
		};
		let max_attempts = if replay.is_some() && method_allowed { self.retry.max_attempts } else { 1 };
		// Capture for the retry loop's per-error gate. Cloning a
		// `Method` is cheap (small enum or `Arc<str>`-backed for
		// custom verbs).
		let method = req.method().clone();

		// H3 dispatch: route through the QuicPool. Single-attempt path
		// (streaming body, non-whitelisted method, or `max_attempts: 1`)
		// short-circuits; the multi-attempt arm below mirrors the TCP
		// path's loop using the per-request `Bytes` replay snapshot.
		#[cfg(feature = "h3")]
		if let Dispatch::Quic(quic) = &self.dispatch {
			if max_attempts <= 1 {
				return self.send_one_attempt_h3(req).await;
			}
			let (parts, _orig) = req.into_parts();
			let replay = replay.expect("max_attempts > 1 implies replay snapshot");
			return self.dispatch_h3_with_retry(parts, replay, max_attempts, quic).await;
		}

		// Streaming or non-retryable-method TCP path: single attempt,
		// original body, no clones.
		if max_attempts <= 1 {
			return self.send_one_attempt_tcp(req).await;
		}

		// Retryable path: rebuild the request from `(method, uri,
		// version, headers)` + replay bytes on every attempt. The
		// original `Extensions` is intentionally dropped — middleware-
		// set extensions don't survive retries (a fresh hyper request
		// can't carry the inbound `OnUpgrade` future, and other
		// extensions are typically per-request and shouldn't leak).
		let (parts, _orig_body) = req.into_parts();
		let replay = replay.expect("max_attempts > 1 path requires replay snapshot");
		let client = self.tcp_client_or_panic();

		let mut last_err: Option<Error> = None;
		for attempt in 1..=max_attempts {
			let req_attempt =
				http::Request::from_parts(clone_parts_for_retry(&parts), Body::Static(replay.clone()));
			// Per `spec/crates/engine.md` § _Error classification_,
			// upstream 4xx/5xx (incl. 503/429) are not retry-eligible —
			// they are complete responses, and a retry would duplicate
			// the request. The `Retry-After` header is forwarded to the
			// client unchanged via the response pass-through below.
			match client.request(req_attempt).await {
				Ok(resp) => {
					let (parts, incoming) = resp.into_parts();
					let body = Body::Stream(Box::pin(IncomingAdapter::new(incoming)));
					return Ok(L7FetchOutput::Response(http::Response::from_parts(parts, body)));
				}
				Err(e) => {
					let err = Error::upstream(UpstreamReason::Unreachable).with_source(e);
					tracing::debug!(
						attempt,
						max_attempts,
						version = ?self.version,
						"upstream request failed",
					);
					if attempt >= max_attempts || !err.is_retryable_in(&method) {
						return Err(err);
					}
					let delay = self.retry.backoff.delay_for_attempt(attempt + 1);
					if !delay.is_zero() {
						tokio::time::sleep(delay).await;
					}
					last_err = Some(err);
				}
			}
		}
		Err(last_err.expect("retry loop runs at least once"))
	}
}

impl HttpProxyFetch {
	/// Borrow the TCP-family client. Panics for the QUIC dispatch path —
	/// the H3 arm short-circuits before the retry loop reaches this
	/// helper, so the panic is unreachable in any well-typed code path.
	fn tcp_client_or_panic(&self) -> &Arc<ProxyClient> {
		match &self.dispatch {
			Dispatch::Tcp(c) => c,
			#[cfg(feature = "h3")]
			Dispatch::Quic(_) => unreachable!(
				"H3 dispatch routes through send_one_attempt_h3 above; tcp helper is unreachable",
			),
		}
	}

	/// Single-attempt TCP-family path used when retry is disabled
	/// (`max_attempts == 1`, streaming body, or method not in
	/// whitelist). Skips the snapshot + clone work.
	async fn send_one_attempt_tcp(&self, req: Request) -> Result<L7FetchOutput, Error> {
		// Elapsed from call entry includes the connect time for connections
		// that need to dial — the pooled client dials inside `request`.
		// This is "request total elapsed including connect" rather than
		// a pure connect measurement.
		let client = self.tcp_client_or_panic();
		let start = std::time::Instant::now();
		let resp = client.request(req).await.map_err(|e| {
			tracing::debug!(error = ?e, version = ?self.version, "upstream request failed");
			Error::upstream(UpstreamReason::Unreachable).with_source(e)
		})?;
		metrics::histogram!("vane.upstream.connect.duration_ms", "kind" => "http_proxy")
			.record(start.elapsed().as_secs_f64() * 1000.0);
		let (parts, incoming) = resp.into_parts();
		// spec/crates/engine.md `spec/crates/engine.md` § _Concrete fetches_: never collect into `Body::Static`.
		let body = Body::Stream(Box::pin(IncomingAdapter::new(incoming)));
		Ok(L7FetchOutput::Response(http::Response::from_parts(parts, body)))
	}

	/// Multi-attempt H3 dispatch. Mirrors the TCP retry loop's shape:
	/// rebuild the request from `(method, uri, version, headers)` plus
	/// the cached `Bytes` snapshot on every attempt, run
	/// `send_one_attempt_h3`, retry whenever
	/// [`Error::is_retryable`] permits and `attempt < max_attempts`.
	/// `quic_pool::evict` runs inside `send_one_attempt_h3` on dead
	/// pool entries; the next loop iteration re-dials through
	/// `get_or_dial`'s cache-miss path.
	#[cfg(feature = "h3")]
	async fn dispatch_h3_with_retry(
		&self,
		parts: http::request::Parts,
		replay: Bytes,
		max_attempts: u32,
		_quic: &QuicDispatchState,
	) -> Result<L7FetchOutput, Error> {
		let mut last_err: Option<Error> = None;
		for attempt in 1..=max_attempts {
			let req_attempt =
				http::Request::from_parts(clone_parts_for_retry(&parts), Body::Static(replay.clone()));
			match self.send_one_attempt_h3(req_attempt).await {
				Ok(out) => return Ok(out),
				Err(err) => {
					tracing::debug!(
						attempt,
						max_attempts,
						version = ?self.version,
						"upstream h3 request failed",
					);
					if attempt >= max_attempts || !err.is_retryable_in(&parts.method) {
						return Err(err);
					}
					let delay = self.retry.backoff.delay_for_attempt(attempt + 1);
					if !delay.is_zero() {
						tokio::time::sleep(delay).await;
					}
					last_err = Some(err);
				}
			}
		}
		Err(last_err.expect("retry loop runs at least once"))
	}

	/// Single-attempt QUIC-family path. Resolves the dispatch's
	/// `host:port` to a `SocketAddr`, composes the per-request `QuicFingerprint`,
	/// acquires the pooled `h3::client::SendRequest` (dialing on miss),
	/// and runs one request / response round-trip with the response
	/// body wrapped in `Body::Stream(Box::pin(H3Body::new(...)))` per
	/// `spec/crates/engine.md` § _Body streaming_ +
	/// `spec/crates/engine.md` § _Concrete fetches_.
	#[cfg(feature = "h3")]
	async fn send_one_attempt_h3(&self, req: Request) -> Result<L7FetchOutput, Error> {
		use http_body::Body as _;

		let Dispatch::Quic(quic) = &self.dispatch else {
			unreachable!("send_one_attempt_h3 called on non-QUIC dispatch")
		};

		let start = std::time::Instant::now();

		// Per-fetch hickory resolver — same code path as the TCP family
		// (which threads `args.dns` into hyper-util's connector). H3
		// has no equivalent connector layer, so the dial composes the
		// resolved `IpAddr` with the upstream's static port directly.
		let ip = quic
			.resolver
			.resolve_first_ip(&quic.host)
			.await
			.map_err(|e| Error::upstream(UpstreamReason::DnsFailure).with_source(e))?;
		let addr = std::net::SocketAddr::new(ip, quic.port);

		let fp = crate::fetch::quic_pool::QuicFingerprint { addr, tls: quic.tls_fp.clone() };
		let entry = match tokio::time::timeout(
			quic.connect_timeout,
			crate::fetch::quic_pool::get_or_dial(fp.clone(), &quic.sni, Arc::clone(&quic.rustls_cfg)),
		)
		.await
		{
			Ok(Ok(e)) => e,
			Ok(Err(e)) => return Err(e),
			Err(_) => {
				return Err(Error::upstream(UpstreamReason::Unreachable).with_source(std::io::Error::new(
					std::io::ErrorKind::TimedOut,
					format!("h3 upstream connect timeout ({:?})", quic.connect_timeout),
				)));
			}
		};
		metrics::histogram!("vane.upstream.connect.duration_ms", "kind" => "http_proxy_h3")
			.record(start.elapsed().as_secs_f64() * 1000.0);

		// h3 wants `http::Request<()>` for the headers half; the body
		// goes via `send_data` on the returned stream. Pin the
		// request version to HTTP/3 — h3's `SendRequest::send_request`
		// rejects requests whose version isn't HTTP/3 (the inbound
		// version on the executor side is whatever the listener
		// negotiated, often HTTP/1.1; that's transport-free above the
		// fetch boundary, so we override here to the upstream's
		// transport-required version).
		//
		// Strip the inbound `Host` header. After URI rewrite the URI's
		// authority is the upstream `host:port`; an inbound H1 client's
		// `Host: client.example` would now contradict it, and h3's
		// `Header::request` rejects the pair as `ContradictedAuthority`
		// (RFC 9114 §4.3.1). h3 carries authority via the `:authority`
		// pseudo-header derived from `uri.authority()`, so the inbound
		// `Host` header is redundant on the H3 path either way.
		let (mut parts, body) = req.into_parts();
		parts.version = http::Version::HTTP_3;
		parts.headers.remove(http::header::HOST);
		let req_headers = http::Request::from_parts(parts, ());

		let mut send_request = entry.send_request.clone();
		let mut stream = match send_request.send_request(req_headers).await {
			Ok(s) => s,
			Err(e) => {
				// Dead pool entry — evict so the next request re-dials.
				crate::fetch::quic_pool::evict(&fp);
				return Err(
					Error::upstream(UpstreamReason::Unreachable)
						.with_source(std::io::Error::other(format!("h3 send_request: {e}"))),
				);
			}
		};

		// Pump the request body. `poll_frame` yields `Frame<Bytes>`
		// items; data frames feed `send_data`, trailer frames feed
		// `send_trailers`. `Body::Empty` and zero-byte Static bodies
		// produce no frames; the stream is closed via `finish` in
		// either case.
		let mut body = Box::pin(body);
		loop {
			let next = std::future::poll_fn(|cx| body.as_mut().poll_frame(cx)).await;
			match next {
				Some(Ok(frame)) => {
					if let Some(data) = frame.data_ref()
						&& let Err(e) = stream.send_data(data.clone()).await
					{
						return Err(
							Error::upstream(UpstreamReason::Unreachable)
								.with_source(std::io::Error::other(format!("h3 send_data: {e}"))),
						);
					} else if let Some(trailers) = frame.trailers_ref()
						&& let Err(e) = stream.send_trailers(trailers.clone()).await
					{
						return Err(
							Error::upstream(UpstreamReason::Unreachable)
								.with_source(std::io::Error::other(format!("h3 send_trailers: {e}"))),
						);
					}
				}
				Some(Err(e)) => {
					return Err(
						Error::upstream(UpstreamReason::Unreachable)
							.with_source(std::io::Error::other(format!("request body read: {e}"))),
					);
				}
				None => break,
			}
		}
		if let Err(e) = stream.finish().await {
			return Err(
				Error::upstream(UpstreamReason::Unreachable)
					.with_source(std::io::Error::other(format!("h3 finish: {e}"))),
			);
		}

		let resp_head = match stream.recv_response().await {
			Ok(r) => r,
			Err(e) => {
				return Err(
					Error::upstream(UpstreamReason::Unreachable)
						.with_source(std::io::Error::other(format!("h3 recv_response: {e}"))),
				);
			}
		};

		// Normalise the response version. h3 sets it to HTTP/3.0, but
		// vane's L7 path is transport-free above the fetch boundary —
		// the listener-side encoder serialises whatever version it
		// negotiated regardless of `resp.version()`. hyper's H1
		// encoder, however, panics on an HTTP/3.0 response (it has
		// no wire format for that version). Pin to HTTP/1.1 so any
		// downstream encoder accepts it.
		let (mut resp_parts, _empty) = resp_head.into_parts();
		resp_parts.version = http::Version::HTTP_11;
		let body = Body::from_producer(h3_body::H3Body::new(h3_body::ClientStreamSource::new(stream)));
		Ok(L7FetchOutput::Response(http::Response::from_parts(resp_parts, body)))
	}
}

/// Split an `args.upstream` `host:port` string into its parts. The
/// returned host has surrounding brackets stripped (`[::1]` → `::1`)
/// so the resolver's IP-literal short-circuit reaches `IpAddr::parse`.
/// Returns the host owned (callers wrap it into `Arc<str>`); port is
/// validated as `u16`.
#[cfg(feature = "h3")]
fn split_host_port(upstream: &str) -> Result<(String, u16), String> {
	let (host_part, port_part) =
		upstream.rsplit_once(':').ok_or_else(|| "missing port".to_string())?;
	let host = host_part.strip_prefix('[').and_then(|s| s.strip_suffix(']')).unwrap_or(host_part);
	if host.is_empty() {
		return Err("empty host".to_string());
	}
	let port = port_part.parse::<u16>().map_err(|e| format!("invalid port: {e}"))?;
	Ok((host.to_owned(), port))
}

/// `http::request::Parts` doesn't implement `Clone` because
/// `Extensions` may hold non-`Clone` types. Rebuild the parts the
/// retry loop needs from the four fields that are individually
/// `Clone`. `Extensions` is dropped on purpose — middleware-set
/// extensions don't survive retries.
fn clone_parts_for_retry(p: &http::request::Parts) -> http::request::Parts {
	let (mut new, _body) = http::Request::new(()).into_parts();
	new.method = p.method.clone();
	new.uri = p.uri.clone();
	new.version = p.version;
	new.headers = p.headers.clone();
	new
}

/// Build the per-instance pooled client. The connector accepts both
/// `http://` and `https://` URIs so a single `Client` handles
/// cleartext and TLS upstreams; the connector's `enable_http1` /
/// `enable_http2` toggles drive the ALPN list, and the legacy
/// builder's `http2_only` flag pins the post-handshake driver.
///
/// `hyper-rustls` rejects a pre-populated `alpn_protocols` on the
/// `ClientConfig` it receives (the connector builder reserves that
/// field for its own use), so the per-version ALPN restriction goes
/// through `enable_httpN` here, not through cloning the cached
/// `ClientConfig`.
fn build_client(
	version: UpstreamVersion,
	tls: Option<&UpstreamTls>,
	dns: &DnsConfig,
) -> Client<HttpsConnector<HttpConnector<HickoryDnsResolver>>, Body> {
	let tls_cfg = match tls {
		Some(t) => Arc::clone(&t.client_config),
		// Cleartext path never reaches the rustls handshake; supply a
		// minimal default config so `HttpsConnectorBuilder` is happy.
		// The connector picks the cleartext branch the moment it sees
		// an `http://` URI.
		None => Arc::new(
			rustls::ClientConfig::builder()
				.with_root_certificates(rustls::RootCertStore::empty())
				.with_no_client_auth(),
		),
	};

	// Resolver is built per Client; spec does not require global sharing
	// and (version, tls, dns) tuples are bounded in production.
	// Hickory's TTL cache lives inside this resolver instance.
	let resolver = HickoryDnsResolver::build(dns).expect("build hickory resolver");
	let mut http = HttpConnector::new_with_resolver(resolver);
	// Permit https:// URIs through the inner connector — TLS is wrapped
	// by hyper-rustls one layer up. Mirrors `HttpConnector::new`'s
	// posture for the GaiResolver path.
	http.enforce_http(false);

	let connector_with_protocols =
		hyper_rustls::HttpsConnectorBuilder::new().with_tls_config((*tls_cfg).clone()).https_or_http();
	let https = match version {
		UpstreamVersion::Auto => {
			connector_with_protocols.enable_http1().enable_http2().wrap_connector(http)
		}
		UpstreamVersion::Http1 => connector_with_protocols.enable_http1().wrap_connector(http),
		UpstreamVersion::Http2 => connector_with_protocols.enable_http2().wrap_connector(http),
		// `Http3` is dispatched via the QuicPool, never through `build_client`.
		// The factory short-circuits before reaching here, so this arm is
		// unreachable in practice; keep it for exhaustiveness.
		#[cfg(feature = "h3")]
		UpstreamVersion::Http3 => {
			unreachable!("build_client is the TCP path; H3 dispatch goes through QuicPool")
		}
	};

	let mut builder = Client::builder(TokioExecutor::new());
	match version {
		// Auto + Http1: hyper-util's legacy client defaults to H1.
		// On TLS the connector restricts ALPN to `http/1.1` for
		// `Http1`; on cleartext H1 is the default (no H2 upgrade
		// path on plain TCP).
		UpstreamVersion::Auto | UpstreamVersion::Http1 => {}
		UpstreamVersion::Http2 => {
			// Prior-knowledge h2c on cleartext, ALPN-h2 on TLS.
			builder.http2_only(true);
		}
		#[cfg(feature = "h3")]
		UpstreamVersion::Http3 => {
			unreachable!("build_client is the TCP path; H3 dispatch goes through QuicPool")
		}
	}
	builder.build(https)
}

/// Fork point for `args.upstream_kind` (injected by the alias-
/// resolution layer in `vane_core::rule::TerminateSpec`, see
/// `spec/crates/engine.md` § _Concrete fetches_): socket-based aliases produce
/// `"tcp"`; the `cgi` alias produces `"cgi"`. Hand-rolled rules
/// without an alias fall through to the socket path for backwards
/// compatibility, but anything else is a hard error so
/// misconfiguration surfaces at link time rather than as a
/// misleading "missing upstream" downstream.
fn dispatch_upstream_kind(args: &serde_json::Value) -> Option<Result<FetchInst, FactoryError>> {
	match args.get("upstream_kind").and_then(serde_json::Value::as_str) {
		#[cfg(feature = "cgi")]
		Some("cgi") => Some(crate::fetch::cgi::factory(args)),
		#[cfg(not(feature = "cgi"))]
		Some("cgi") => Some(Err(FactoryError::Invalid(
			"upstream_kind 'cgi' requires the 'cgi' cargo feature, which is not active in this build"
				.to_string(),
		))),
		Some("tcp") | None => None,
		Some(other) => Some(Err(FactoryError::Invalid(format!(
			"args.upstream_kind must be 'tcp' or 'cgi' (or absent for backwards-compat with hand-written socket rules) — got {other:?}",
		)))),
	}
}

/// Args parser exposed as a registry-friendly factory.
///
/// Args shape:
///
/// ```json
/// {
///   "upstream": "host:port",
///   "version":  "auto" | "h1" | "h2" | "h3",
///   "tls": {
///     "verify_hostname":      "api.example.com",
///     "insecure_skip_verify": false
///   }
/// }
/// ```
///
/// `version` defaults to `"auto"`. `"h3"` is reserved for the future
/// `h3` cargo feature; factories on builds without it return an
/// error pointing operators at the right rebuild flag. `tls` is
/// optional — absent means cleartext upstream.
///
/// # Errors
/// Returns [`FactoryError`] when `upstream` is missing/empty, when
/// `version` is not one of the four accepted strings, when
/// `version: "h3"` is requested on a build without the `h3` feature,
/// or when the TLS client config fails to build.
pub fn factory(
	args: &serde_json::Value,
	crl_cache: Option<&Arc<crate::tls::CrlCache>>,
) -> Result<FetchInst, FactoryError> {
	if let Some(out) = dispatch_upstream_kind(args) {
		return out;
	}
	let upstream = args.get("upstream").and_then(serde_json::Value::as_str).ok_or_else(|| {
		FactoryError::Invalid("missing args.upstream (string \"host:port\")".to_string())
	})?;
	if upstream.is_empty() {
		return Err(FactoryError::Invalid("args.upstream must not be empty".to_string()));
	}
	let version = parse_version_arg(args)?;
	let tls = parse_tls_args(upstream, args.get("tls"), crl_cache)
		.map_err(|e| FactoryError::Invalid(format!("args.tls: {e}")))?;
	let dns =
		parse_dns_args(args.get("dns")).map_err(|e| FactoryError::Invalid(format!("args.dns: {e}")))?;

	if matches!(version, UpstreamVersion::Auto) && tls.is_none() {
		// Cleartext has no ALPN to negotiate on, so `auto` collapses
		// to H1. Surface the degradation so operators who actually
		// wanted h2c add `version: "h2"` explicitly.
		tracing::warn!(
			upstream,
			"cleartext upstream + version=auto: no ALPN to negotiate, falling back to h1; \
			 set version: h2 explicitly for prior-knowledge h2c",
		);
	}

	let retry = crate::fetch::retry::parse(args.get("retry"))
		.map_err(|e| FactoryError::Invalid(format!("args.retry: {e}")))?;

	#[cfg(feature = "h3")]
	if matches!(version, UpstreamVersion::Http3) {
		return build_h3_dispatch(args, upstream, version, tls, &dns, retry);
	}

	// TCP family — compute the cache key. The connector wires ALPN
	// via `enable_http1` / `enable_http2`, which is `version`-driven,
	// so we patch the version-specific ALPN list into the parsed TLS
	// fingerprint here (parse_tls_args has no `version` to consult).
	// Cleartext upstreams keep `tls: None` and still share by version.
	let alpn_protocols = match version {
		UpstreamVersion::Auto => vec![b"h2".to_vec(), b"http/1.1".to_vec()],
		UpstreamVersion::Http1 => vec![b"http/1.1".to_vec()],
		UpstreamVersion::Http2 => vec![b"h2".to_vec()],
		// Unreachable — the H3 branch above already returned.
		#[cfg(feature = "h3")]
		UpstreamVersion::Http3 => unreachable!("H3 dispatch returns above"),
	};
	let tls_fp = tls.as_ref().map(|t| {
		let mut fp = t.fingerprint.clone();
		fp.alpn_protocols = alpn_protocols;
		fp
	});
	let client_fp = ClientFingerprint { version, tls: tls_fp, dns: dns.clone() };
	let tls_for_build = tls.clone();
	let dns_for_build = dns.clone();
	let client = crate::fetch::client_cache::get_or_build(client_fp, move || {
		build_client(version, tls_for_build.as_ref(), &dns_for_build)
	});

	let scheme = if tls.is_some() { http::uri::Scheme::HTTPS } else { http::uri::Scheme::HTTP };
	let authority: http::uri::Authority = upstream.parse().map_err(|e| {
		FactoryError::Invalid(format!("args.upstream {upstream:?}: invalid authority: {e}"))
	})?;

	Ok(FetchInst::L7(Arc::new(HttpProxyFetch {
		version,
		scheme,
		authority,
		dispatch: Dispatch::Tcp(client),
		retry: Arc::new(retry),
	})))
}

/// Plug `FetchKind::HttpProxy` into a `FetchFactories` registry. The
/// `crl_cache` is captured by the registered closure so each factory
/// invocation routes through the daemon-wide cache.
pub fn register(factories: &mut FetchFactories, crl_cache: Option<Arc<crate::tls::CrlCache>>) {
	factories.register(FetchKind::HttpProxy, move |args| factory(args, crl_cache.as_ref()));
}

/// Parse `args.version` (default `"auto"`) into [`UpstreamVersion`].
/// `"h3"` on a build without the `h3` feature surfaces the rebuild
/// hint at factory time so operators don't get a less specific link
/// error downstream.
fn parse_version_arg(args: &serde_json::Value) -> Result<UpstreamVersion, FactoryError> {
	match args.get("version").and_then(serde_json::Value::as_str).unwrap_or("auto") {
		"auto" => Ok(UpstreamVersion::Auto),
		"h1" => Ok(UpstreamVersion::Http1),
		"h2" => Ok(UpstreamVersion::Http2),
		#[cfg(feature = "h3")]
		"h3" => Ok(UpstreamVersion::Http3),
		#[cfg(not(feature = "h3"))]
		"h3" => Err(FactoryError::Invalid(
			"version 'h3' requires the 'h3' cargo feature, which is not active in this build".to_string(),
		)),
		other => Err(FactoryError::Invalid(format!(
			"args.version must be one of 'auto' / 'h1' / 'h2' / 'h3' — got {other:?}"
		))),
	}
}

/// Build the H3 dispatch state and wrap it as a [`FetchInst::L7`].
/// TLS is mandatory (RFC 9114 mandates QUIC + TLS 1.3); cleartext H3
/// is rejected at factory time. The rustls config is cloned and ALPN
/// is pinned to `[b"h3"]` since the QUIC pool embeds ALPN into the
/// rustls config (vs the hyper-rustls connector's `enable_httpN`).
#[cfg(feature = "h3")]
fn build_h3_dispatch(
	args: &serde_json::Value,
	upstream: &str,
	version: UpstreamVersion,
	tls: Option<UpstreamTls>,
	dns: &DnsConfig,
	retry: RetryPolicy,
) -> Result<FetchInst, FactoryError> {
	let tls = tls.ok_or_else(|| {
		FactoryError::Invalid("version 'h3' requires args.tls (h3 mandates QUIC + TLS 1.3)".to_string())
	})?;
	let mut h3_rustls: rustls::ClientConfig = (*tls.client_config).clone();
	h3_rustls.alpn_protocols = vec![b"h3".to_vec()];
	let h3_rustls = Arc::new(h3_rustls);
	let mut tls_fp = tls.fingerprint.clone();
	tls_fp.alpn_protocols = vec![b"h3".to_vec()];
	let connect_timeout = match args.get("connect_timeout").and_then(serde_json::Value::as_str) {
		Some(s) => crate::fetch::retry::parse_duration(s)
			.map_err(|e| FactoryError::Invalid(format!("args.connect_timeout: {e}")))?,
		None => H3_CONNECT_TIMEOUT_DEFAULT,
	};
	let resolver = HickoryDnsResolver::build(dns)
		.map_err(|e| FactoryError::Invalid(format!("args.dns hickory build: {e}")))?;
	let (host, port) = split_host_port(upstream)
		.map_err(|e| FactoryError::Invalid(format!("args.upstream {upstream:?}: {e}")))?;
	let dispatch = Dispatch::Quic(QuicDispatchState {
		rustls_cfg: h3_rustls,
		sni: Arc::from(tls.verify_hostname.as_str()),
		tls_fp,
		connect_timeout,
		resolver: Arc::new(resolver),
		host: Arc::from(host.as_str()),
		port,
	});
	let authority: http::uri::Authority = upstream.parse().map_err(|e| {
		FactoryError::Invalid(format!("args.upstream {upstream:?}: invalid authority: {e}"))
	})?;
	Ok(FetchInst::L7(Arc::new(HttpProxyFetch {
		version,
		scheme: http::uri::Scheme::HTTPS,
		authority,
		dispatch,
		retry: Arc::new(retry),
	})))
}

#[cfg(test)]
mod tests {
	use super::*;

	fn install_crypto() {
		crate::crypto::install_default_provider();
	}

	#[test]
	fn factory_rejects_missing_upstream() {
		install_crypto();
		match factory(&serde_json::json!({}), None) {
			Ok(_) => panic!("must reject missing upstream"),
			Err(e) => assert!(e.message().contains("upstream"), "{}", e.message()),
		}
	}

	#[test]
	fn factory_rejects_empty_upstream() {
		install_crypto();
		match factory(&serde_json::json!({ "upstream": "" }), None) {
			Ok(_) => panic!("must reject empty upstream"),
			Err(e) => assert!(e.message().contains("must not be empty"), "{}", e.message()),
		}
	}

	#[test]
	fn factory_rejects_tls_with_insecure_skip_verify_when_env_unset() {
		// Per the spec's master-switch contract: `insecure_skip_verify`
		// in config alone is insufficient — VANE_ALLOW_INSECURE_UPSTREAM=1
		// has to be set in the daemon env. The unit-test environment
		// never sets that, so the factory must refuse the config.
		install_crypto();
		let Err(FactoryError::Invalid(msg)) = factory(
			&serde_json::json!({
				"upstream": "127.0.0.1:9443",
				"tls": { "insecure_skip_verify": true, "verify_hostname": "localhost" },
			}),
			None,
		) else {
			panic!("factory must reject insecure tls config without env opt-in");
		};
		assert!(msg.contains("VANE_ALLOW_INSECURE_UPSTREAM"), "error names env var: {msg}");
	}

	#[cfg(not(feature = "h3"))]
	#[test]
	fn factory_rejects_version_h3_without_feature() {
		install_crypto();
		let Err(FactoryError::Invalid(msg)) = factory(
			&serde_json::json!({
				"upstream": "127.0.0.1:9443",
				"version": "h3",
			}),
			None,
		) else {
			panic!("h3 must be rejected on builds without the feature");
		};
		assert!(msg.contains("h3"), "error names the missing feature: {msg}");
	}

	#[cfg(feature = "h3")]
	#[test]
	fn factory_rejects_h3_without_tls() {
		install_crypto();
		// H3 mandates QUIC + TLS 1.3 (RFC 9114) — the factory rejects
		// `version: "h3"` without `args.tls` even with the cargo
		// feature enabled.
		let Err(FactoryError::Invalid(msg)) = factory(
			&serde_json::json!({
				"upstream": "127.0.0.1:9443",
				"version": "h3",
			}),
			None,
		) else {
			panic!("h3 without tls must be rejected");
		};
		assert!(msg.contains("h3") && msg.contains("tls"), "error names h3 + tls: {msg}");
	}

	#[cfg(feature = "h3")]
	#[test]
	fn factory_rejects_h3_with_insecure_skip_verify_when_env_unset() {
		// Same master-switch contract as the H1/H2 path: H3 + TLS with
		// `insecure_skip_verify` is rejected without the env opt-in.
		install_crypto();
		let Err(FactoryError::Invalid(msg)) = factory(
			&serde_json::json!({
				"upstream": "127.0.0.1:9443",
				"version": "h3",
				"tls": { "insecure_skip_verify": true, "verify_hostname": "localhost" },
			}),
			None,
		) else {
			panic!("h3 + insecure must be rejected without env opt-in");
		};
		assert!(msg.contains("VANE_ALLOW_INSECURE_UPSTREAM"), "error names env var: {msg}");
	}

	#[test]
	fn factory_rejects_unknown_version() {
		install_crypto();
		let Err(FactoryError::Invalid(msg)) = factory(
			&serde_json::json!({
				"upstream": "127.0.0.1:9443",
				"version": "h7",
			}),
			None,
		) else {
			panic!("unknown version must be rejected");
		};
		assert!(msg.contains("auto") && msg.contains("h1"), "{msg}");
	}

	#[test]
	fn factory_accepts_explicit_h1_version() {
		install_crypto();
		let result = factory(
			&serde_json::json!({
				"upstream": "127.0.0.1:9443",
				"version": "h1",
			}),
			None,
		);
		assert!(result.is_ok(), "h1 version must build");
	}

	#[test]
	fn factory_accepts_explicit_h2_cleartext() {
		install_crypto();
		let result = factory(
			&serde_json::json!({
				"upstream": "127.0.0.1:9443",
				"version": "h2",
			}),
			None,
		);
		assert!(result.is_ok(), "h2 cleartext (h2c) must build");
	}

	#[cfg(feature = "h3")]
	#[test]
	fn split_host_port_accepts_ipv4() {
		assert_eq!(split_host_port("127.0.0.1:443").unwrap(), ("127.0.0.1".to_owned(), 443));
	}

	#[cfg(feature = "h3")]
	#[test]
	fn split_host_port_strips_ipv6_brackets() {
		assert_eq!(split_host_port("[::1]:8443").unwrap(), ("::1".to_owned(), 8443));
	}

	#[cfg(feature = "h3")]
	#[test]
	fn split_host_port_accepts_dns_name() {
		assert_eq!(
			split_host_port("api.example.com:443").unwrap(),
			("api.example.com".to_owned(), 443),
		);
	}

	#[cfg(feature = "h3")]
	#[test]
	fn split_host_port_rejects_no_port() {
		assert!(split_host_port("127.0.0.1").is_err());
	}

	#[cfg(feature = "h3")]
	#[test]
	fn split_host_port_rejects_bad_port() {
		assert!(split_host_port("127.0.0.1:abc").is_err());
	}

	#[cfg(feature = "h3")]
	#[test]
	fn split_host_port_rejects_empty_host() {
		assert!(split_host_port(":443").is_err());
	}
}
