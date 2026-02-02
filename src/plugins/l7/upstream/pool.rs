/* src/plugins/l7/upstream/pool.rs */

use super::tls_verifier::NoVerifier;
use crate::common::sys::lifecycle::Error;
use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::{HttpConnector, dns::Name};
use hyper_util::rt::{TokioExecutor, TokioTimer};
use once_cell::sync::Lazy;
use rustls::ClientConfig;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tower_service::Service;

#[derive(Clone)]
pub struct VaneResolver;

impl Service<Name> for VaneResolver {
	type Response = std::vec::IntoIter<SocketAddr>;
	type Error = std::io::Error;
	type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

	fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
		Poll::Ready(Ok(()))
	}

	fn call(&mut self, name: Name) -> Self::Future {
		let host = name.as_str().to_owned();
		Box::pin(async move {
			// Call Global Resolver
			let ips = crate::layers::l4::resolver::resolve_domain_to_ips(&host).await;

			if ips.is_empty() {
				return Err(std::io::Error::other(format!(
					"Vane DNS lookup returned no IPs for {host}"
				)));
			}

			// Hyper expects SocketAddr (IP + Port).
			// DNS only gives IPs. We use port 0 as a placeholder;
			// HttpConnector will replace it with the actual port (80/443).
			let addrs: Vec<SocketAddr> = ips.into_iter().map(|ip| SocketAddr::new(ip, 0)).collect();

			Ok(addrs.into_iter())
		})
	}
}

// --- Client Types ---
pub type HttpClient = Client<HttpsConnector<HttpConnector<VaneResolver>>, BoxBody<Bytes, Error>>;

// --- Global Pools ---
pub static GLOBAL_SECURE_CLIENT: Lazy<HttpClient> = Lazy::new(|| build_client(false));
pub static GLOBAL_INSECURE_CLIENT: Lazy<HttpClient> = Lazy::new(|| build_client(true));

fn build_client(skip_verify: bool) -> HttpClient {
	let idle_timeout_s = envflag::get::<u64>("UPSTREAM_POOL_IDLE_TIMEOUT", 90);
	let max_idle = envflag::get::<usize>("UPSTREAM_POOL_MAX_IDLE", 32);
	let keepalive_s = envflag::get::<u64>("UPSTREAM_KEEPALIVE_INTERVAL", 30);
	let h2_stream_window = envflag::get::<u32>("UPSTREAM_H2_STREAM_WINDOW", 2_097_152);
	let h2_conn_window = envflag::get::<u32>("UPSTREAM_H2_CONN_WINDOW", 2_097_152);

	// 1. Build Base HttpConnector with Custom DNS
	let mut http_connector = HttpConnector::new_with_resolver(VaneResolver);
	http_connector.enforce_http(false); // Allow HTTPS wrapping

	// 2. Configure TLS
	let https_connector = if skip_verify {
		let mut config = ClientConfig::builder()
			.with_root_certificates(rustls::RootCertStore::empty())
			.with_no_client_auth();

		config
			.dangerous()
			.set_certificate_verifier(Arc::new(NoVerifier));

		hyper_rustls::HttpsConnectorBuilder::new()
			.with_tls_config(config)
			.https_or_http()
			.enable_all_versions()
			.wrap_connector(http_connector) // Wrap our custom DNS connector
	} else {
		// Use native roots if possible
		hyper_rustls::HttpsConnectorBuilder::new()
			.with_native_roots()
			.expect("Failed to load native roots")
			.https_or_http()
			.enable_all_versions()
			.wrap_connector(http_connector) // Wrap our custom DNS connector
	};

	// 3. Build Client
	// Explicitly supply TokioTimer for pool management
	Client::builder(TokioExecutor::new())
		.timer(TokioTimer::new())
		.pool_idle_timeout(Duration::from_secs(idle_timeout_s))
		.pool_max_idle_per_host(max_idle)
		.http2_keep_alive_interval(Some(Duration::from_secs(keepalive_s)))
		// Tuning HTTP/2 Window Sizes for High Throughput
		.http2_initial_stream_window_size(Some(h2_stream_window))
		.http2_initial_connection_window_size(Some(h2_conn_window))
		.build(https_connector)
}
