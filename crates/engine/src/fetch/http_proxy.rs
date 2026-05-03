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
//! Permitted values mirror `spec/architecture/09-config.md` § _Rule
//! schema_ (`version` row):
//!
//! | `version` | TLS upstream                | Cleartext upstream     |
//! | --------- | --------------------------- | ---------------------- |
//! | `auto`    | ALPN: prefer `h2`, fall H1  | H1 (no ALPN; warn)     |
//! | `h1`      | ALPN: only `http/1.1`       | H1                     |
//! | `h2`      | ALPN: only `h2`             | h2c (prior knowledge)  |
//! | `h3`      | ALPN: only `h3` (TLS req'd) | rejected (h3 mandates QUIC TLS) |
//!
//! See `spec/architecture/05-terminator.md` § _`HttpProxy`_,
//! `spec/architecture/07-l7.md` § _H1 / H2 paths_, § _Architecture: TCP / QUIC separation_,
//! and `spec/architecture/08-tls.md` § _TLS library: rustls only_.

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
	upstream: Arc<str>,
	version: UpstreamVersion,
	scheme: &'static str,
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
/// stay live until quinn's idle timeout retires them.
#[cfg(feature = "h3")]
struct QuicDispatchState {
	rustls_cfg: Arc<rustls::ClientConfig>,
	sni: Arc<str>,
	tls_fp: crate::fetch::client_cache::TlsConfigFingerprint,
}

/// Spec default for `HttpProxyFetch.connect_timeout`
/// (`spec/architecture/07-l7.md` § _Timeouts (proposal)_). Caps the
/// H3 dial half (DNS + UDP bind + QUIC handshake + h3 negotiation);
/// pool-hit fast paths short-circuit before reaching it. Pool entries
/// retired by `quinn`'s own idle timeout are also lazily re-dialed
/// against this same ceiling on the next miss.
// TODO(s3-02-followup): make this configurable via args.connect_timeout.
#[cfg(feature = "h3")]
const H3_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

#[async_trait]
impl L7Fetch for HttpProxyFetch {
	async fn fetch(
		&self,
		mut req: Request,
		_conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<L7FetchOutput, Error> {
		// Compose the full upstream URI so the dispatch path routes by
		// scheme + authority. For TCP (hyper-util Client) the connector
		// reads `http://` / `https://` to pick cleartext vs TLS; for QUIC
		// (h3 client) the scheme + authority become :scheme / :authority
		// pseudo-headers so the upstream sees the rewritten target.
		let path_and_query =
			req.uri().path_and_query().map_or("/", http::uri::PathAndQuery::as_str).to_string();
		let new_uri = format!("{}://{}{}", self.scheme, self.upstream, path_and_query);
		*req.uri_mut() =
			new_uri.parse().map_err(|e| Error::protocol("upstream uri rewrite").with_source(e))?;

		// H3 dispatch: route through the QuicPool. Retry on H3 streams is
		// not yet supported — the fetch dispatches a single attempt and
		// surfaces any error directly. Per
		// `spec/architecture/07-l7.md` § _Retry policy_, retry requires
		// request-body replay; streaming bodies skip retry on the TCP
		// path too. Adding H3-aware retry is a separate task.
		// TODO(s3-02-followup): implement retry on the H3 dispatch path
		// when the request body is `Body::Static` / `Body::Empty`.
		#[cfg(feature = "h3")]
		if let Dispatch::Quic(_) = &self.dispatch {
			return self.send_one_attempt_h3(req).await;
		}

		// Snapshot the request body for replay. `Body::Stream` is
		// one-shot — it collapses retry to a single attempt
		// regardless of `max_attempts`. `Body::Static` clones via
		// `Bytes` refcount; `Body::Empty` replays as zero-length
		// `Bytes`. Per `spec/architecture/05-terminator.md`
		// § _Retry buffering_, this is the `opportunistic` rule:
		// streaming bodies skip retry quietly. `force` buffering is
		// implemented earlier in the lower pass — by the time the
		// fetch sees the request, a `force` policy has already
		// converted the body to `Body::Static`.
		let method_allowed = self.retry.methods.contains(req.method());
		let replay: Option<Bytes> = match req.body() {
			Body::Static(b) => Some(b.clone()),
			Body::Empty => Some(Bytes::new()),
			Body::Stream(_) => None,
		};
		let max_attempts = if replay.is_some() && method_allowed { self.retry.max_attempts } else { 1 };

		// Streaming or non-retryable-method path: single attempt,
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
			// TODO(retry-after): respect upstream Retry-After on 503/429.
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
					if attempt >= max_attempts || !err.is_retryable() {
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
		// 07-l7.md § _`HttpProxyFetch` commits to streaming response
		// bodies_: never collect into `Body::Static`.
		let body = Body::Stream(Box::pin(IncomingAdapter::new(incoming)));
		Ok(L7FetchOutput::Response(http::Response::from_parts(parts, body)))
	}

	/// Single-attempt QUIC-family path. Resolves `self.upstream` to a
	/// `SocketAddr`, composes the per-request `QuicFingerprint`,
	/// acquires the pooled `h3::client::SendRequest` (dialing on miss),
	/// and runs one request / response round-trip with the response
	/// body wrapped in `Body::Stream(Box::pin(H3Body::new(...)))` per
	/// `spec/architecture/07-l7.md` § _Upstream-H3 send path_ +
	/// § _`HttpProxyFetch` commits to streaming response bodies_.
	#[cfg(feature = "h3")]
	#[allow(clippy::too_many_lines)]
	async fn send_one_attempt_h3(&self, req: Request) -> Result<L7FetchOutput, Error> {
		use http_body::Body as _;

		let Dispatch::Quic(quic) = &self.dispatch else {
			unreachable!("send_one_attempt_h3 called on non-QUIC dispatch")
		};

		let start = std::time::Instant::now();

		// Per-request DNS resolve via tokio's system resolver. The
		// hickory resolver wired into the TCP path only feeds
		// `hyper_util`'s connector and is not reachable from quinn
		// directly; integrating it here is a separate task.
		// TODO(s3-02-followup): route H3 dial through the per-fetch
		// hickory resolver so `args.dns` overrides apply.
		let addr = match tokio::net::lookup_host(self.upstream.as_ref()).await {
			Ok(mut iter) => iter.next().ok_or_else(|| {
				Error::upstream(UpstreamReason::Unreachable).with_source(std::io::Error::new(
					std::io::ErrorKind::AddrNotAvailable,
					format!("no addresses for upstream {:?}", self.upstream),
				))
			})?,
			Err(e) => {
				return Err(Error::upstream(UpstreamReason::Unreachable).with_source(e));
			}
		};

		let fp = crate::fetch::quic_pool::QuicFingerprint { addr, tls: quic.tls_fp.clone() };
		let entry = match tokio::time::timeout(
			H3_CONNECT_TIMEOUT,
			crate::fetch::quic_pool::get_or_dial(fp.clone(), &quic.sni, Arc::clone(&quic.rustls_cfg)),
		)
		.await
		{
			Ok(Ok(e)) => e,
			Ok(Err(e)) => return Err(e),
			Err(_) => {
				return Err(Error::upstream(UpstreamReason::Unreachable).with_source(std::io::Error::new(
					std::io::ErrorKind::TimedOut,
					"h3 upstream connect timeout (5s)",
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
		let body = Body::Stream(Box::pin(crate::h3_body::H3Body::new(
			crate::h3_body::ClientStreamSource::new(stream),
		)));
		Ok(L7FetchOutput::Response(http::Response::from_parts(resp_parts, body)))
	}
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
pub fn factory(args: &serde_json::Value) -> Result<FetchInst, FactoryError> {
	let upstream = args
		.get("upstream")
		.and_then(serde_json::Value::as_str)
		.ok_or_else(|| FactoryError("missing args.upstream (string \"host:port\")".to_string()))?;
	if upstream.is_empty() {
		return Err(FactoryError("args.upstream must not be empty".to_string()));
	}
	let version_str = args.get("version").and_then(serde_json::Value::as_str).unwrap_or("auto");
	let version = match version_str {
		"auto" => UpstreamVersion::Auto,
		"h1" => UpstreamVersion::Http1,
		"h2" => UpstreamVersion::Http2,
		#[cfg(feature = "h3")]
		"h3" => UpstreamVersion::Http3,
		#[cfg(not(feature = "h3"))]
		"h3" => {
			return Err(FactoryError(
				"version 'h3' requires the 'h3' cargo feature, which is not active in this build"
					.to_string(),
			));
		}
		other => {
			return Err(FactoryError(format!(
				"args.version must be one of 'auto' / 'h1' / 'h2' / 'h3' — got {other:?}"
			)));
		}
	};
	let tls = parse_tls_args(upstream, args.get("tls"))
		.map_err(|e| FactoryError(format!("args.tls: {e}")))?;

	let dns = parse_dns_args(args.get("dns")).map_err(|e| FactoryError(format!("args.dns: {e}")))?;

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
		.map_err(|e| FactoryError(format!("args.retry: {e}")))?;

	// H3 dispatch — TLS is mandatory (RFC 9114 mandates QUIC + TLS
	// 1.3); reject cleartext H3 at factory time. Build a separate
	// rustls config with ALPN pinned to `[b"h3"]` since the QUIC pool
	// embeds ALPN into the rustls config (vs the hyper-rustls
	// connector's `enable_httpN`).
	#[cfg(feature = "h3")]
	if matches!(version, UpstreamVersion::Http3) {
		let tls = tls.ok_or_else(|| {
			FactoryError("version 'h3' requires args.tls (h3 mandates QUIC + TLS 1.3)".to_string())
		})?;
		// Clone the parsed `ClientConfig` and patch ALPN — the cached
		// `Arc<ClientConfig>` from `build_client_config` has no ALPN set
		// (the TCP path's hyper-rustls connector reserves that field).
		let mut h3_rustls: rustls::ClientConfig = (*tls.client_config).clone();
		h3_rustls.alpn_protocols = vec![b"h3".to_vec()];
		let h3_rustls = Arc::new(h3_rustls);
		let mut tls_fp = tls.fingerprint.clone();
		tls_fp.alpn_protocols = vec![b"h3".to_vec()];
		let dispatch = Dispatch::Quic(QuicDispatchState {
			rustls_cfg: h3_rustls,
			sni: Arc::from(tls.verify_hostname.as_str()),
			tls_fp,
		});
		return Ok(FetchInst::L7(Arc::new(HttpProxyFetch {
			upstream: Arc::from(upstream),
			version,
			scheme: "https",
			dispatch,
			retry: Arc::new(retry),
		})));
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

	let scheme = if tls.is_some() { "https" } else { "http" };

	Ok(FetchInst::L7(Arc::new(HttpProxyFetch {
		upstream: Arc::from(upstream),
		version,
		scheme,
		dispatch: Dispatch::Tcp(client),
		retry: Arc::new(retry),
	})))
}

/// Plug `FetchKind::HttpProxy` into a `FetchFactories` registry.
pub fn register(factories: &mut FetchFactories) {
	factories.register(FetchKind::HttpProxy, factory);
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
		match factory(&serde_json::json!({})) {
			Ok(_) => panic!("must reject missing upstream"),
			Err(e) => assert!(e.0.contains("upstream"), "{}", e.0),
		}
	}

	#[test]
	fn factory_rejects_empty_upstream() {
		install_crypto();
		match factory(&serde_json::json!({ "upstream": "" })) {
			Ok(_) => panic!("must reject empty upstream"),
			Err(e) => assert!(e.0.contains("must not be empty"), "{}", e.0),
		}
	}

	#[test]
	fn factory_accepts_tls_with_insecure_skip_verify() {
		install_crypto();
		let result = factory(&serde_json::json!({
			"upstream": "127.0.0.1:9443",
			"tls": { "insecure_skip_verify": true, "verify_hostname": "localhost" },
		}));
		assert!(result.is_ok(), "factory must accept insecure tls config");
	}

	#[cfg(not(feature = "h3"))]
	#[test]
	fn factory_rejects_version_h3_without_feature() {
		install_crypto();
		let Err(FactoryError(msg)) = factory(&serde_json::json!({
			"upstream": "127.0.0.1:9443",
			"version": "h3",
		})) else {
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
		let Err(FactoryError(msg)) = factory(&serde_json::json!({
			"upstream": "127.0.0.1:9443",
			"version": "h3",
		})) else {
			panic!("h3 without tls must be rejected");
		};
		assert!(msg.contains("h3") && msg.contains("tls"), "error names h3 + tls: {msg}");
	}

	#[cfg(feature = "h3")]
	#[test]
	fn factory_accepts_h3_with_tls() {
		install_crypto();
		let result = factory(&serde_json::json!({
			"upstream": "127.0.0.1:9443",
			"version": "h3",
			"tls": { "insecure_skip_verify": true, "verify_hostname": "localhost" },
		}));
		assert!(result.is_ok(), "h3 + tls must build: {:?}", result.err());
	}

	#[test]
	fn factory_rejects_unknown_version() {
		install_crypto();
		let Err(FactoryError(msg)) = factory(&serde_json::json!({
			"upstream": "127.0.0.1:9443",
			"version": "h7",
		})) else {
			panic!("unknown version must be rejected");
		};
		assert!(msg.contains("auto") && msg.contains("h1"), "{msg}");
	}

	#[test]
	fn factory_accepts_explicit_h1_version() {
		install_crypto();
		let result = factory(&serde_json::json!({
			"upstream": "127.0.0.1:9443",
			"version": "h1",
		}));
		assert!(result.is_ok(), "h1 version must build");
	}

	#[test]
	fn factory_accepts_explicit_h2_cleartext() {
		install_crypto();
		let result = factory(&serde_json::json!({
			"upstream": "127.0.0.1:9443",
			"version": "h2",
		}));
		assert!(result.is_ok(), "h2 cleartext (h2c) must build");
	}
}
