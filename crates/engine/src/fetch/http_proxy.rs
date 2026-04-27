//! `HttpProxyFetch` — pooled, ALPN-aware reverse-proxy fetch.
//!
//! Forwards the decoded `Request` to a configured upstream HTTP
//! server and returns its `Response` to the executor. The dial path
//! is owned by `hyper_util::client::legacy::Client` over a
//! `hyper_rustls::HttpsConnector<HttpConnector>`: per-authority
//! connection pooling, ALPN-driven H1/H2 negotiation on TLS, and
//! cleartext h2c via prior knowledge when the rule pins
//! `version: "h2"` without TLS.
//!
//! The `version` field selects the upstream's HTTP version posture.
//! Permitted values mirror `spec/architecture/09-config.md` § _Rule
//! schema_ (`version` row):
//!
//! | `version` | TLS upstream                | Cleartext upstream    |
//! | --------- | --------------------------- | --------------------- |
//! | `auto`    | ALPN: prefer `h2`, fall H1  | H1 (no ALPN; warn)    |
//! | `h1`      | ALPN: only `http/1.1`       | H1                    |
//! | `h2`      | ALPN: only `h2`             | h2c (prior knowledge) |
//! | `h3`      | rejected at factory time (no `h3` cargo feature yet)         |
//!
//! See `spec/architecture/05-terminator.md` § _`HttpProxy`_,
//! `spec/architecture/07-l7.md` § _H1 / H2 paths_, and
//! `spec/architecture/08-tls.md` § _TLS library: rustls only_.

use std::sync::Arc;

use async_trait::async_trait;
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use vane_core::{
	Body, ConnContext, Error, FetchKind, FlowCtx, L7Fetch, L7FetchOutput, Request, UpstreamReason,
};

use crate::body_adapter::IncomingAdapter;
use crate::factories::{FactoryError, FetchFactories};
use crate::fetch::upstream::{UpstreamTls, parse_tls_args};
use crate::flow_graph::FetchInst;

/// Upstream HTTP-version posture. Pinned at factory time from
/// `args.version`. `Http3` is reserved for an `h3` cargo feature; the
/// factory rejects it on builds without that feature.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum UpstreamVersion {
	Auto,
	Http1,
	Http2,
}

/// Reverse-proxy fetch backed by a per-instance pooled
/// `legacy::Client`. One `HttpProxyFetch` is one logical upstream;
/// the pool inside the client multiplexes on `(scheme, authority)`.
pub struct HttpProxyFetch {
	upstream: Arc<str>,
	version: UpstreamVersion,
	scheme: &'static str,
	client: Client<HttpsConnector<HttpConnector>, Body>,
}

#[async_trait]
impl L7Fetch for HttpProxyFetch {
	async fn fetch(
		&self,
		mut req: Request,
		_conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<L7FetchOutput, Error> {
		// Compose the full upstream URI so the legacy client routes by
		// scheme + authority. The connector reads `http://` / `https://`
		// to pick cleartext vs TLS; the pool keys by authority.
		let path_and_query =
			req.uri().path_and_query().map_or("/", http::uri::PathAndQuery::as_str).to_string();
		let new_uri = format!("{}://{}{}", self.scheme, self.upstream, path_and_query);
		*req.uri_mut() =
			new_uri.parse().map_err(|e| Error::protocol("upstream uri rewrite").with_source(e))?;

		let resp = self.client.request(req).await.map_err(|e| {
			tracing::debug!(error = ?e, version = ?self.version, "upstream request failed");
			Error::upstream(UpstreamReason::Unreachable).with_source(e)
		})?;

		let (parts, incoming) = resp.into_parts();
		// 07-l7.md § _`HttpProxyFetch` commits to streaming response
		// bodies_: never collect into `Body::Static`.
		let body = Body::Stream(Box::pin(IncomingAdapter::new(incoming)));
		Ok(L7FetchOutput::Response(http::Response::from_parts(parts, body)))
	}
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
) -> Client<HttpsConnector<HttpConnector>, Body> {
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

	let connector_with_protocols =
		hyper_rustls::HttpsConnectorBuilder::new().with_tls_config((*tls_cfg).clone()).https_or_http();
	let https = match version {
		UpstreamVersion::Auto => connector_with_protocols.enable_http1().enable_http2().build(),
		UpstreamVersion::Http1 => connector_with_protocols.enable_http1().build(),
		UpstreamVersion::Http2 => connector_with_protocols.enable_http2().build(),
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

	let scheme = if tls.is_some() { "https" } else { "http" };
	let client = build_client(version, tls.as_ref());

	Ok(FetchInst::L7(Arc::new(HttpProxyFetch {
		upstream: Arc::from(upstream),
		version,
		scheme,
		client,
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
