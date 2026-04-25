//! `HttpProxyFetch` — H1→H1 reverse-proxy fetch.
//!
//! Forwards the decoded `Request` to a configured upstream HTTP/1.x
//! server via [`hyper_util::client::legacy::Client`], and returns the
//! upstream's `Response` to the executor for the L7 response middleware
//! chain + `Terminator::WriteHttpResponse`. Cleartext only this stage;
//! HTTPS upstreams (rustls-wrapped connector) and H2/H3 client paths
//! land later.
//!
//! See `spec/architecture/07-l7.md` § _H1 path_,
//! `spec/architecture/05-terminator.md` § _`HttpProxy`_. Feature: S1-19.

use std::sync::Arc;

use async_trait::async_trait;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use vane_core::{
	Body, ConnContext, Error, FetchKind, FlowCtx, L7Fetch, L7FetchOutput, Request, UpstreamReason,
};

use crate::body_adapter::IncomingAdapter;
use crate::factories::{FactoryError, FetchFactories};
use crate::flow_graph::FetchInst;

/// Reverse-proxy Fetch backed by a `hyper_util::client::legacy::Client`.
/// One `Client` per `HttpProxyFetch` instance — its internal connection
/// pool is keyed by authority, so the lower pass's hash-cons of stateless
/// fetches (two rules with the same `upstream`) collapses cleanly into
/// one shared pool.
pub struct HttpProxyFetch {
	client: Client<HttpConnector, Body>,
	/// `host:port` literal substituted into every forwarded request's
	/// URI. Stored as `Arc<str>` so the per-request format string clone
	/// is cheap.
	upstream: Arc<str>,
}

#[async_trait]
impl L7Fetch for HttpProxyFetch {
	async fn fetch(
		&self,
		mut req: Request,
		_conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<L7FetchOutput, Error> {
		// Rewrite the request URI's scheme + authority to point at the
		// configured upstream. `hyper_util::Client` routes by URI authority
		// (07-l7.md § _H1 path_ → "TCP pooling is delegated entirely to
		// hyper_util::client::legacy::Client, which keys its internal pool
		// by authority"). Path and query are preserved verbatim.
		let path_and_query =
			req.uri().path_and_query().map_or("/", http::uri::PathAndQuery::as_str).to_string();
		let new_uri = format!("http://{}{}", self.upstream, path_and_query);
		*req.uri_mut() =
			new_uri.parse().map_err(|e| Error::protocol("upstream uri rewrite").with_source(e))?;

		let resp = self.client.request(req).await.map_err(classify_client_error)?;

		let (parts, incoming) = resp.into_parts();
		// 07-l7.md § _`HttpProxyFetch` commits to streaming response bodies_:
		// upstream response bodies are always wrapped in `Body::Stream(...)`.
		// We never collect into `Body::Static` defensively.
		let body = Body::Stream(Box::pin(IncomingAdapter::new(incoming)));
		Ok(L7FetchOutput::Response(http::Response::from_parts(parts, body)))
	}
}

/// Best-effort classification of `hyper_util::client::legacy::Error` into
/// `UpstreamReason`. The legacy Client doesn't expose a stable enum — we
/// stringify and sniff for the common cases. Anything we don't recognise
/// falls back to `UpstreamReason::Unreachable`. S2's retry layer will
/// replace this with a structured branch on hyper's typed errors.
fn classify_client_error(e: hyper_util::client::legacy::Error) -> Error {
	let s = format!("{e:#}").to_lowercase();
	let reason = if s.contains("dns") {
		UpstreamReason::DnsFailure
	} else if s.contains("tls") || s.contains("handshake") {
		UpstreamReason::TlsHandshake
	} else if s.contains("reset") {
		UpstreamReason::ResetMidRequest
	} else {
		// Fallback covers connect-refused, host-unreachable, and anything
		// the matchers above didn't recognise. The legacy Client doesn't
		// expose stable error variants — string sniffing is intentionally
		// loose. S2's retry layer replaces this with structured branching.
		UpstreamReason::Unreachable
	};
	Error::upstream(reason).with_source(e)
}

/// Args parser exposed as a registry-friendly factory.
///
/// Args shape:
///
/// ```json
/// { "upstream": "host:port" }
/// ```
///
/// # Errors
/// Returns [`FactoryError`] when `upstream` is missing, not a string, or
/// empty. Wider validation (literal `host:port` parse, port range) is
/// deferred — `Client::request` produces a pointed `UpstreamReason` at
/// runtime.
pub fn factory(args: &serde_json::Value) -> Result<FetchInst, FactoryError> {
	let upstream = args
		.get("upstream")
		.and_then(serde_json::Value::as_str)
		.ok_or_else(|| FactoryError("missing args.upstream (string \"host:port\")".to_string()))?;
	if upstream.is_empty() {
		return Err(FactoryError("args.upstream must not be empty".to_string()));
	}

	let connector = HttpConnector::new();
	let client: Client<HttpConnector, Body> = Client::builder(TokioExecutor::new()).build(connector);

	Ok(FetchInst::L7(Arc::new(HttpProxyFetch { client, upstream: Arc::from(upstream) })))
}

/// Plug `FetchKind::HttpProxy` into a `FetchFactories` registry.
pub fn register(factories: &mut FetchFactories) {
	factories.register(FetchKind::HttpProxy, factory);
}
