//! `WebSocketUpgrade` — H1→H1 WebSocket reverse-proxy fetch.
//!
//! Architecture P (proxy-style passthrough): vane does **not** speak
//! WebSocket itself. It forwards the client's HTTP/1.1 `Upgrade:
//! websocket` request verbatim to the upstream, awaits upstream's 101
//! Switching Protocols, captures the upgraded upstream IO via
//! `hyper::upgrade::on`, stashes it on `ConnContext.user`, and returns
//! the upstream's 101 (with body replaced by `Body::Empty` — RFC 6455
//! forbids any body on a 101). The client-side upgrade dance + the
//! bidirectional `copy_bidirectional` happens in
//! [`crate::upgrade::drive_h1_server`]'s service-fn after the 101
//! reaches the wire.
//!
//! Consequences of the passthrough design:
//! - vane never inspects WebSocket frames (just bytes after 101).
//! - `Sec-WebSocket-Protocol` / `Sec-WebSocket-Extensions` /
//!   `Sec-WebSocket-Accept` are entirely upstream's responsibility;
//!   vane neither validates nor rewrites them.
//! - The upstream dial path runs through
//!   [`crate::fetch::upstream::dial_upstream`], so an `args.tls`
//!   block flips the upstream socket from cleartext TCP to a
//!   `tokio_rustls::client::TlsStream` (`wss://upstream`). The
//!   client-side scheme is independent — vane terminates either
//!   cleartext WS or WSS at the L4→L7 handshake.
//!
//! See `spec/crates/engine.md` `spec/crates/engine.md` § _Concrete fetches_,
//! `spec/crates/engine.md` `spec/crates/engine.md` § _Concrete fetches_.

use std::sync::Arc;

use async_trait::async_trait;
use hyper_util::rt::TokioIo;
use parking_lot::Mutex;
use vane_core::{
	AsyncReadWrite, Body, ConnContext, Error, FetchKind, FlowCtx, L7Fetch, L7FetchOutput, Request,
	Response, UpstreamReason,
};

use crate::body_adapter::IncomingAdapter;
use crate::factories::{FactoryError, FetchFactories};
use crate::fetch::upstream::{UpstreamTls, dial_upstream, parse_tls_args};
use crate::flow_graph::FetchInst;

/// `ConnContext.user` extension that hands the upgraded upstream IO
/// from the WS fetch to `drive_h1_server`'s service-fn.
///
/// Three constraints stack:
/// - `http::Extensions::insert` requires `T: Clone + Send + Sync +
///   'static`.
/// - The inner IO is a `Box<dyn AsyncReadWrite + Send>` — no `Sync`,
///   no `Clone`.
/// - We need to remove the IO from the stash (consume-on-take) before
///   spawning `copy_bidirectional`.
///
/// `Arc<Mutex<Option<...>>>` resolves all three: `Arc` makes the
/// outer type `Clone` (cheap refcount); `Mutex` adds the `Sync` the
/// inner Box lacks; `Option::take()` consumes-on-take so duplicate
/// extracts return `None`. Wrapping in this newtype keeps the type
/// unique against any other `Arc<Mutex<...>>` an extension consumer
/// might also stash.
#[derive(Clone)]
pub(crate) struct StashedUpstreamUpgrade(
	pub(crate) Arc<Mutex<Option<Box<dyn AsyncReadWrite + Send>>>>,
);

impl StashedUpstreamUpgrade {
	fn new(io: Box<dyn AsyncReadWrite + Send>) -> Self {
		Self(Arc::new(Mutex::new(Some(io))))
	}

	pub(crate) fn take(&self) -> Option<Box<dyn AsyncReadWrite + Send>> {
		self.0.lock().take()
	}
}

pub struct WebSocketUpgradeFetch {
	/// `host:port` literal substituted into the forwarded request URI.
	upstream: Arc<str>,
	/// Optional TLS configuration for the upstream dial. `None` means
	/// cleartext WS upstream; `Some(_)` flips to WSS.
	tls: Option<UpstreamTls>,
}

#[async_trait]
impl L7Fetch for WebSocketUpgradeFetch {
	async fn fetch(
		&self,
		mut req: Request,
		conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<L7FetchOutput, Error> {
		// Same URI rewrite shape as `HttpProxyFetch`: hyper's H1 client
		// composes the request line from the URI's authority, so we
		// substitute the configured upstream and preserve path+query.
		let path_and_query =
			req.uri().path_and_query().map_or("/", http::uri::PathAndQuery::as_str).to_string();
		let new_uri = format!("http://{}{}", self.upstream, path_and_query);
		*req.uri_mut() =
			new_uri.parse().map_err(|e| Error::protocol("ws upstream uri rewrite").with_source(e))?;

		// One-shot H1 dial via the shared upstream helper — supports
		// both cleartext WS and WSS depending on `self.tls`. The
		// pooled `hyper_util::Client` is unsuitable here regardless of
		// scheme because pooled clients close their upgrade channel
		// when they release the connection back to the pool.
		let stream = dial_upstream(&self.upstream, self.tls.as_ref()).await?;
		let io = TokioIo::new(stream);

		let (mut sender, conn_task) =
			hyper::client::conn::http1::handshake::<_, Body>(io).await.map_err(|e| {
				tracing::debug!(?e, "ws upstream handshake failed");
				Error::upstream(UpstreamReason::Unreachable).with_source(e)
			})?;
		// `with_upgrades()` keeps the upgrade channel alive past the
		// response; without it the conn task drops upgrade ownership
		// and the upstream `OnUpgrade` future closes immediately.
		let conn_task = conn_task.with_upgrades();
		tokio::spawn(async move {
			if let Err(e) = conn_task.await {
				tracing::debug!(?e, "ws upstream conn task ended");
			}
		});

		let mut upstream_resp = sender.send_request(req).await.map_err(|e| {
			tracing::debug!(?e, "ws upstream send_request failed");
			Error::upstream(UpstreamReason::Unreachable).with_source(e)
		})?;

		if upstream_resp.status() != http::StatusCode::SWITCHING_PROTOCOLS {
			// Upstream declined the upgrade — forward the body verbatim
			// like a normal H1 response. No upgrade handle to stash;
			// the executor's WriteHttpResponse path takes it from here.
			let (parts, incoming) = upstream_resp.into_parts();
			let body = Body::Stream(Box::pin(IncomingAdapter::new(incoming)));
			return Ok(L7FetchOutput::Response(Response::from_parts(parts, body)));
		}

		// 101 — capture the upgraded upstream IO. After this, the
		// upstream socket is owned by hyper's upgrade machinery; we
		// adopt it via `hyper::upgrade::on(&mut response)` (which
		// pulls the OnUpgrade future out of the response's
		// extensions) and `await` it to get the Upgraded handle.
		let on_upstream = hyper::upgrade::on(&mut upstream_resp);
		let upgraded = on_upstream.await.map_err(|e| {
			tracing::debug!(?e, "ws upstream upgrade await failed");
			Error::upstream(UpstreamReason::Refused).with_source(e)
		})?;

		// `hyper::upgrade::Upgraded` implements hyper's I/O traits;
		// `TokioIo` adapts it to `tokio::io::AsyncRead + AsyncWrite`,
		// which is what `vane_core::AsyncReadWrite` is auto-impl'd for.
		let upstream_io: Box<dyn AsyncReadWrite + Send> = Box::new(TokioIo::new(upgraded));
		conn.user.lock().insert(StashedUpstreamUpgrade::new(upstream_io));

		// Return upstream's 101 line + headers verbatim, but with an
		// empty body — RFC 6455 forbids body bytes on a 101, and any
		// post-status bytes on the upstream socket are now post-upgrade
		// data we must not let hyper interpret.
		let (parts, _body) = upstream_resp.into_parts();
		let resp_for_client = Response::from_parts(parts, Body::Empty);
		Ok(L7FetchOutput::Response(resp_for_client))
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
///     "verify_hostname":      "ws.example.com",
///     "insecure_skip_verify": false
///   }
/// }
/// ```
///
/// `tls` is optional — absent means cleartext WS upstream; present
/// flips the upstream socket to `tokio_rustls` (WSS). The same
/// schema as `HttpProxyFetch`.
///
/// # Errors
/// [`FactoryError`] when `upstream` is missing/empty or when the TLS
/// client config fails to build.
pub fn factory(
	args: &serde_json::Value,
	crl_cache: Option<&Arc<crate::tls::CrlCache>>,
) -> Result<FetchInst, FactoryError> {
	let upstream = args
		.get("upstream")
		.and_then(serde_json::Value::as_str)
		.ok_or_else(|| FactoryError("missing args.upstream (string \"host:port\")".to_string()))?;
	if upstream.is_empty() {
		return Err(FactoryError("args.upstream must not be empty".to_string()));
	}
	let tls = parse_tls_args(upstream, args.get("tls"), crl_cache)
		.map_err(|e| FactoryError(format!("args.tls: {e}")))?;
	Ok(FetchInst::L7(Arc::new(WebSocketUpgradeFetch { upstream: Arc::from(upstream), tls })))
}

/// Plug `FetchKind::WebSocketUpgrade` into a `FetchFactories` registry.
/// The `crl_cache` is captured by the registered closure.
pub fn register(factories: &mut FetchFactories, crl_cache: Option<Arc<crate::tls::CrlCache>>) {
	factories.register(FetchKind::WebSocketUpgrade, move |args| factory(args, crl_cache.as_ref()));
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn factory_rejects_missing_upstream() {
		match factory(&serde_json::json!({}), None) {
			Ok(_) => panic!("must reject missing upstream"),
			Err(e) => assert!(e.0.contains("upstream"), "{}", e.0),
		}
	}

	#[test]
	fn factory_rejects_empty_upstream() {
		match factory(&serde_json::json!({ "upstream": "" }), None) {
			Ok(_) => panic!("must reject empty upstream"),
			Err(e) => assert!(e.0.contains("must not be empty"), "{}", e.0),
		}
	}
}
