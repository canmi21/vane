//! `AcmeChallengeFetch` — the HTTP-01 ACME challenge responder.
//!
//! Per `spec/acme.md` § _HTTP-01 § Case 1_:
//!
//! 1. Extract the token from the request path tail
//!    (`/.well-known/acme-challenge/<token>`).
//! 2. Look up `(Host header, token)` in the registry's pending
//!    table.
//! 3. On hit: respond `200 OK` with `Content-Type:
//!    application/octet-stream` and the key authorisation as the
//!    body.
//! 4. On miss: respond `404 Not Found`. The
//!    `/.well-known/acme-challenge/` namespace is reserved by
//!    `vane` whenever ACME is in use, so falling through to operator
//!    rules would surface ACME plumbing as if it were ordinary 404s.
//!
//! Method + version validation lives in the predicate the compiler
//! injects upstream (per spec: `GET HTTP/1.1`); the fetch trusts
//! whatever request reaches it and only does the lookup-or-404.

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use http::header::{CONTENT_TYPE, HOST};
use http::response;
use vane_core::{Body, ConnContext, Error, FetchKind, FlowCtx, L7Fetch, L7FetchOutput, Request};

use crate::acme::ManagedCertRegistry;
use crate::factories::FetchFactories;
use crate::flow_graph::FetchInst;

const PATH_PREFIX: &str = "/.well-known/acme-challenge/";

/// L7 fetch that resolves `/.well-known/acme-challenge/<token>`
/// against the daemon's [`ManagedCertRegistry`].
///
/// Constructed by the compile lower pass when injecting the
/// challenge route into a plaintext `:80` listener; not reachable
/// through operator-authored rules.
pub struct AcmeChallengeFetch {
	registry: Arc<ManagedCertRegistry>,
}

impl AcmeChallengeFetch {
	#[must_use]
	pub fn new(registry: Arc<ManagedCertRegistry>) -> Self {
		Self { registry }
	}
}

#[async_trait]
impl L7Fetch for AcmeChallengeFetch {
	async fn fetch(
		&self,
		req: Request,
		_conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<L7FetchOutput, Error> {
		let path = req.uri().path();
		let token = path.strip_prefix(PATH_PREFIX).unwrap_or("");
		let host = req
			.headers()
			.get(HOST)
			.and_then(|v| v.to_str().ok())
			.map(host_from_header)
			.unwrap_or_default();

		match self.registry.lookup_http01(&host, token) {
			Some(key_authorization) => respond_200(key_authorization),
			None => respond_404(),
		}
	}
}

/// Strip the optional `:port` suffix from a `Host` header value.
/// `Host: api.example.com:8080` is rare on `:80` validators but
/// the registry's lookup table uses bare hostnames as keys, so
/// strip defensively.
fn host_from_header(raw: &str) -> String {
	raw.split(':').next().unwrap_or("").to_ascii_lowercase()
}

fn respond_200(key_authorization: String) -> Result<L7FetchOutput, Error> {
	let body = Body::Static(Bytes::from(key_authorization));
	let resp = response::Builder::new()
		.status(200)
		.header(CONTENT_TYPE, "application/octet-stream")
		.body(body)
		.map_err(|e| Error::internal(format!("acme challenge response: {e}")))?;
	Ok(L7FetchOutput::Response(resp))
}

fn respond_404() -> Result<L7FetchOutput, Error> {
	let resp = response::Builder::new()
		.status(404)
		.header(CONTENT_TYPE, "text/plain")
		.body(Body::Static(Bytes::from_static(b"acme challenge not found")))
		.map_err(|e| Error::internal(format!("acme challenge 404: {e}")))?;
	Ok(L7FetchOutput::Response(resp))
}

/// Plug `FetchKind::AcmeChallenge` into a `FetchFactories` registry.
/// `registry` is captured by the constructor closure and shared
/// across every `AcmeChallengeFetch` instance the link pass
/// materialises — typically just one per linked graph since the
/// inject pass emits a single fetch node per `FlowGraph` regardless
/// of how many `:80` listeners receive the route.
pub fn register(factories: &mut FetchFactories, registry: Arc<ManagedCertRegistry>) {
	factories.register(FetchKind::AcmeChallenge, move |_args| {
		Ok(FetchInst::L7(Arc::new(AcmeChallengeFetch::new(Arc::clone(&registry)))))
	});
}

#[cfg(test)]
mod tests {
	use std::net::SocketAddr;
	use std::time::Instant;

	use http::HeaderValue;
	use parking_lot::Mutex;
	use tokio_util::sync::CancellationToken;
	use vane_core::flow_log::{FlowLogEvent, FlowLogSink, FlowLogVerbosity, TrajectoryBuilder};
	use vane_core::ir::NodeId;
	use vane_core::{ConnId, Transport};

	struct NullSink;
	impl FlowLogSink for NullSink {
		fn emit(&self, _: FlowLogEvent) {}
	}

	use super::*;
	use crate::acme::AcmeStore;
	use crate::acme::store::{AcmeAccount, LockGuard, StoreError, StoredCert};

	#[derive(Default)]
	struct MockStore;

	#[derive(Debug)]
	struct MockGuard;
	impl LockGuard for MockGuard {}

	#[async_trait]
	impl AcmeStore for MockStore {
		async fn load_account(&self, _: &str) -> Result<Option<AcmeAccount>, StoreError> {
			Ok(None)
		}
		async fn save_account(&self, _: &str, _: &AcmeAccount) -> Result<(), StoreError> {
			Ok(())
		}
		async fn load_cert(&self, _: &str) -> Result<Option<StoredCert>, StoreError> {
			Ok(None)
		}
		async fn save_cert(&self, _: &str, _: &StoredCert) -> Result<(), StoreError> {
			Ok(())
		}
		async fn list_cert_snis(&self) -> Result<Vec<String>, StoreError> {
			Ok(Vec::new())
		}
		async fn lock(&self, _: &str) -> Result<Box<dyn LockGuard>, StoreError> {
			Ok(Box::new(MockGuard))
		}
	}

	fn make_request(host: &str, path: &str) -> Request {
		let mut req =
			http::Request::builder().method("GET").uri(path).body(Body::Empty).expect("build request");
		req.headers_mut().insert(HOST, HeaderValue::from_str(host).unwrap());
		req
	}

	fn conn_ctx() -> Arc<ConnContext> {
		let addr: SocketAddr = "127.0.0.1:0".parse().expect("parse addr");
		Arc::new(ConnContext {
			id: ConnId(0),
			remote: addr,
			local: addr,
			transport: Transport::Tcp,
			entered_at: Instant::now(),
			tls: Mutex::new(None),
			http_version: std::sync::OnceLock::new(),
			user: Mutex::new(http::Extensions::new()),
		})
	}

	fn flow_ctx(conn_id: ConnId) -> FlowCtx {
		FlowCtx {
			span: tracing::Span::none(),
			log: Arc::new(NullSink),
			cancel: CancellationToken::new(),
			verbosity: FlowLogVerbosity::Trajectory,
			trajectory: TrajectoryBuilder::new(conn_id, NodeId::new(0), 0),
		}
	}

	async fn fetch_response(fetch: &AcmeChallengeFetch, req: Request) -> http::Response<Body> {
		let conn = conn_ctx();
		let mut ctx = flow_ctx(conn.id);
		match fetch.fetch(req, &conn, &mut ctx).await.expect("fetch") {
			L7FetchOutput::Response(r) => r,
			L7FetchOutput::Tunnel(_) => unreachable!("AcmeChallenge fetch never produces a tunnel"),
		}
	}

	fn body_to_bytes(body: Body) -> Vec<u8> {
		match body {
			Body::Static(b) => b.to_vec(),
			Body::Empty => Vec::new(),
			Body::Stream(_) => panic!("AcmeChallenge fetch always produces Body::Static or Empty"),
		}
	}

	#[tokio::test]
	async fn returns_200_with_key_authorization_for_registered_token() {
		let registry =
			ManagedCertRegistry::open(Arc::new(MockStore) as Arc<dyn AcmeStore>).await.unwrap();
		registry.register_http01("api.example.com", "tok-XYZ".into(), "ka-ABC".into());
		let fetch = AcmeChallengeFetch::new(registry);
		let req = make_request("api.example.com", "/.well-known/acme-challenge/tok-XYZ");
		let resp = fetch_response(&fetch, req).await;
		assert_eq!(resp.status(), 200);
		assert_eq!(
			resp.headers().get(CONTENT_TYPE).map(http::HeaderValue::as_bytes),
			Some(b"application/octet-stream" as &[u8]),
		);
		let body = body_to_bytes(resp.into_body());
		assert_eq!(body, b"ka-ABC");
	}

	#[tokio::test]
	async fn returns_404_when_token_unknown() {
		let registry =
			ManagedCertRegistry::open(Arc::new(MockStore) as Arc<dyn AcmeStore>).await.unwrap();
		let fetch = AcmeChallengeFetch::new(registry);
		let req = make_request("api.example.com", "/.well-known/acme-challenge/missing-tok");
		let resp = fetch_response(&fetch, req).await;
		assert_eq!(resp.status(), 404);
	}

	#[tokio::test]
	async fn host_header_with_port_strips_to_bare_name() {
		let registry =
			ManagedCertRegistry::open(Arc::new(MockStore) as Arc<dyn AcmeStore>).await.unwrap();
		registry.register_http01("api.example.com", "tok".into(), "ka".into());
		let fetch = AcmeChallengeFetch::new(registry);
		// CA validators usually omit the port but defensively handle it.
		let req = make_request("api.example.com:80", "/.well-known/acme-challenge/tok");
		let resp = fetch_response(&fetch, req).await;
		assert_eq!(resp.status(), 200);
	}

	#[tokio::test]
	async fn host_header_case_is_normalised() {
		let registry =
			ManagedCertRegistry::open(Arc::new(MockStore) as Arc<dyn AcmeStore>).await.unwrap();
		registry.register_http01("api.example.com", "tok".into(), "ka".into());
		let fetch = AcmeChallengeFetch::new(registry);
		let req = make_request("API.EXAMPLE.COM", "/.well-known/acme-challenge/tok");
		let resp = fetch_response(&fetch, req).await;
		assert_eq!(resp.status(), 200);
	}

	#[tokio::test]
	async fn cross_host_token_does_not_leak() {
		// Two SNIs share the same token (statistically unlikely but
		// constructible); a request to the wrong Host must miss.
		let registry =
			ManagedCertRegistry::open(Arc::new(MockStore) as Arc<dyn AcmeStore>).await.unwrap();
		registry.register_http01("api.example.com", "tok".into(), "ka-api".into());
		let fetch = AcmeChallengeFetch::new(registry);
		let req = make_request("admin.example.com", "/.well-known/acme-challenge/tok");
		let resp = fetch_response(&fetch, req).await;
		assert_eq!(resp.status(), 404);
	}

	#[tokio::test]
	async fn missing_token_in_path_returns_404() {
		let registry =
			ManagedCertRegistry::open(Arc::new(MockStore) as Arc<dyn AcmeStore>).await.unwrap();
		let fetch = AcmeChallengeFetch::new(registry);
		let req = make_request("api.example.com", "/.well-known/acme-challenge/");
		let resp = fetch_response(&fetch, req).await;
		assert_eq!(resp.status(), 404);
	}
}
