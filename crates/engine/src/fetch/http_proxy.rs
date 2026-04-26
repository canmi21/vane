//! `HttpProxyFetch` — H1→H1 reverse-proxy fetch.
//!
//! Forwards the decoded `Request` to a configured upstream HTTP/1.x
//! server and returns the upstream's `Response` to the executor for
//! the L7 response middleware chain + `Terminator::WriteHttpResponse`.
//!
//! Stage 1 supports both cleartext and TLS upstreams. The dial path
//! is unified through [`crate::fetch::upstream::dial_upstream`]: an
//! `args.tls` block flips the connection from a raw `TcpStream` to a
//! `tokio_rustls::client::TlsStream`. The hyper H1 client handshake
//! is otherwise identical.
//!
//! **Performance note**: this implementation opens one TCP/TLS
//! connection per request. The previous cleartext-only path used
//! `hyper_util::client::legacy::Client` with an authority-keyed
//! connection pool; integrating the TLS connector while preserving
//! that pool requires `hyper-util::HttpsConnector`, which the
//! workspace doesn't pull in (the spec bans `hyper-tls`). The pool
//! returns alongside the H2-upstream chunk via a custom
//! tower-service connector. Until then, latency-sensitive deployments
//! should be aware that each request pays a fresh handshake.
//!
//! See `spec/architecture/07-l7.md` § _H1 path_,
//! `spec/architecture/05-terminator.md` § _`HttpProxy`_,
//! `spec/architecture/08-tls.md` § _Provider banking_. Feature: S1-19.

use std::sync::Arc;

use async_trait::async_trait;
use hyper_util::rt::TokioIo;
use vane_core::{
	Body, ConnContext, Error, FetchKind, FlowCtx, L7Fetch, L7FetchOutput, Request, UpstreamReason,
};

use crate::body_adapter::IncomingAdapter;
use crate::factories::{FactoryError, FetchFactories};
use crate::fetch::upstream::{UpstreamTls, dial_upstream, parse_tls_args};
use crate::flow_graph::FetchInst;

/// Reverse-proxy Fetch. One instance per `(upstream, tls)` pair —
/// `parse_tls_args` builds the `Arc<rustls::ClientConfig>` once at
/// factory time, so the per-request work is just the dial + hyper
/// handshake.
pub struct HttpProxyFetch {
	/// `host:port` literal substituted into every forwarded request's
	/// URI. Stored as `Arc<str>` so the per-request format string clone
	/// is cheap.
	upstream: Arc<str>,
	/// Optional TLS configuration. `None` means cleartext upstream
	/// (the original Stage 1 behaviour); `Some(_)` flips the dial path
	/// to wrap the TCP socket with `tokio_rustls`.
	tls: Option<UpstreamTls>,
}

#[async_trait]
impl L7Fetch for HttpProxyFetch {
	async fn fetch(
		&self,
		mut req: Request,
		_conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<L7FetchOutput, Error> {
		// Rewrite the request URI's authority + path so the H1 client
		// composes a request line targeting the upstream. Scheme is
		// kept as `http` regardless of `tls` because hyper's H1 client
		// only uses the URI for the request line — TLS happens at the
		// transport layer below it.
		let path_and_query =
			req.uri().path_and_query().map_or("/", http::uri::PathAndQuery::as_str).to_string();
		let new_uri = format!("http://{}{}", self.upstream, path_and_query);
		*req.uri_mut() =
			new_uri.parse().map_err(|e| Error::protocol("upstream uri rewrite").with_source(e))?;

		let stream = dial_upstream(&self.upstream, self.tls.as_ref()).await?;
		let io = TokioIo::new(stream);
		let (mut sender, conn) =
			hyper::client::conn::http1::handshake::<_, Body>(io).await.map_err(|e| {
				tracing::debug!(?e, "upstream h1 handshake failed");
				Error::upstream(UpstreamReason::Unreachable).with_source(e)
			})?;
		// Drive the connection task in the background — when the
		// response body finishes streaming, hyper closes the
		// connection and the task ends. Errors here are diagnostic
		// only; the response is already in flight to the client.
		tokio::spawn(async move {
			if let Err(e) = conn.await {
				tracing::debug!(?e, "upstream h1 conn task ended");
			}
		});

		let resp = sender.send_request(req).await.map_err(|e| {
			tracing::debug!(?e, "upstream h1 send_request failed");
			Error::upstream(UpstreamReason::Unreachable).with_source(e)
		})?;

		let (parts, incoming) = resp.into_parts();
		// 07-l7.md § _`HttpProxyFetch` commits to streaming response
		// bodies_: upstream response bodies are always wrapped in
		// `Body::Stream(...)`. Never collected into `Body::Static`
		// defensively.
		let body = Body::Stream(Box::pin(IncomingAdapter::new(incoming)));
		Ok(L7FetchOutput::Response(http::Response::from_parts(parts, body)))
	}
}

/// Args parser exposed as a registry-friendly factory.
///
/// Args shape:
///
/// ```json
/// {
///   "upstream": "host:port",
///   "tls": {
///     "verify_hostname":      "api.example.com",
///     "insecure_skip_verify": false
///   }
/// }
/// ```
///
/// `tls` is optional — absent means cleartext. `verify_hostname`
/// defaults to the host portion of `upstream`.
/// `insecure_skip_verify: true` is **testing-only** and skips
/// certificate validation entirely.
///
/// # Errors
/// [`FactoryError`] when `upstream` is missing/empty or when the TLS
/// client config fails to build (typically a system trust store
/// load failure on `insecure_skip_verify: false`).
pub fn factory(args: &serde_json::Value) -> Result<FetchInst, FactoryError> {
	let upstream = args
		.get("upstream")
		.and_then(serde_json::Value::as_str)
		.ok_or_else(|| FactoryError("missing args.upstream (string \"host:port\")".to_string()))?;
	if upstream.is_empty() {
		return Err(FactoryError("args.upstream must not be empty".to_string()));
	}
	let tls = parse_tls_args(upstream, args.get("tls"))
		.map_err(|e| FactoryError(format!("args.tls: {e}")))?;
	Ok(FetchInst::L7(Arc::new(HttpProxyFetch { upstream: Arc::from(upstream), tls })))
}

/// Plug `FetchKind::HttpProxy` into a `FetchFactories` registry.
pub fn register(factories: &mut FetchFactories) {
	factories.register(FetchKind::HttpProxy, factory);
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn factory_rejects_missing_upstream() {
		match factory(&serde_json::json!({})) {
			Ok(_) => panic!("must reject missing upstream"),
			Err(e) => assert!(e.0.contains("upstream"), "{}", e.0),
		}
	}

	#[test]
	fn factory_rejects_empty_upstream() {
		match factory(&serde_json::json!({ "upstream": "" })) {
			Ok(_) => panic!("must reject empty upstream"),
			Err(e) => assert!(e.0.contains("must not be empty"), "{}", e.0),
		}
	}

	#[test]
	fn factory_accepts_tls_with_insecure_skip_verify() {
		// Cheap factory-level check — building the rustls ClientConfig
		// with the no-verify verifier doesn't touch the system trust
		// store, so this works without `rustls::crypto::install_default`.
		let result = factory(&serde_json::json!({
			"upstream": "127.0.0.1:9443",
			"tls": { "insecure_skip_verify": true, "verify_hostname": "localhost" },
		}));
		assert!(result.is_ok(), "factory must accept insecure tls config");
	}
}
