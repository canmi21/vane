//! Runtime dispatch: `impl L7Fetch for HttpProxyFetch` plus the
//! per-request send / receive helpers for both transport families.
//!
//! The TCP retry loop and the H3 body-pump + response-recv live here;
//! everything in this file runs on the per-request hot path. Build-
//! time construction (args parsing, client builders) lives in
//! [`super::factory`].

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use vane_core::{
	Body, ConnContext, Error, FlowCtx, L7Fetch, L7FetchOutput, Request, UpstreamReason,
};

use super::{Dispatch, HttpProxyFetch};
use crate::body_adapter::IncomingAdapter;
use crate::fetch::client_cache::ProxyClient;
use crate::fetch::pool;

#[async_trait]
impl L7Fetch for HttpProxyFetch {
	async fn fetch(
		&self,
		mut req: Request,
		_conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<L7FetchOutput, Error> {
		// `max_concurrent_per_host` gate, per `spec/crates/engine.md`
		// § _Exhaustion defaults (per upstream)_. Hyper-util's legacy
		// client has no native semaphore, so the limiter is enforced
		// here. The permit is held across the URI rewrite, retry
		// loop, H3 dispatch, and the moment the response is handed
		// back — releasing earlier would let saturated upstreams
		// silently exceed the documented cap. Saturated waits beyond
		// `pool::CONNECT_TIMEOUT` surface as `Unreachable` (503).
		let _limit_permit = pool::limiter().acquire(&self.authority).await?;

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

		// Strip hop-by-hop headers (RFC 7230 §6.1) before any retry
		// snapshot of the request. `HttpProxyFetch` does not handle
		// WebSocket upgrades — that path is owned by
		// `WebSocketUpgradeFetch`. Even so, the strip is WS-aware so
		// future fetch flavours that share this helper inherit the
		// correct posture; for the present caller, no `Upgrade:
		// websocket` will be present and the exception is a no-op.
		// See `fetch/hop_by_hop.rs`.
		crate::fetch::hop_by_hop::strip_hop_by_hop_request(req.headers_mut());

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
					let (mut parts, incoming) = resp.into_parts();
					// Strip upstream's hop-by-hop headers before
					// handing the response to the client. The 101
					// switching-protocols exception is gated on
					// status; `HttpProxyFetch` never produces a 101
					// (upgrades go through `WebSocketUpgradeFetch`),
					// but we evaluate the predicate honestly so the
					// future plumbing matches.
					let is_101 = parts.status == http::StatusCode::SWITCHING_PROTOCOLS;
					crate::fetch::hop_by_hop::strip_hop_by_hop_response(&mut parts.headers, is_101);
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
		let (mut parts, incoming) = resp.into_parts();
		// Strip upstream hop-by-hop before relaying. See the retry
		// loop above for the rationale; this is the single-attempt
		// twin.
		let is_101 = parts.status == http::StatusCode::SWITCHING_PROTOCOLS;
		crate::fetch::hop_by_hop::strip_hop_by_hop_response(&mut parts.headers, is_101);
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
		_quic: &super::QuicDispatchState,
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
		// H3 responses carry their own hop-by-hop set: even though
		// QUIC subsumes Transfer-Encoding etc. on the wire, an
		// upstream may still emit `Connection: x-leak` if it is a
		// non-conforming gateway. Treat the H3 path identically to
		// the H1/H2 paths.
		let is_101 = resp_parts.status == http::StatusCode::SWITCHING_PROTOCOLS;
		crate::fetch::hop_by_hop::strip_hop_by_hop_response(&mut resp_parts.headers, is_101);
		let body = Body::from_producer(h3_body::H3Body::new(h3_body::ClientStreamSource::new(stream)));
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
