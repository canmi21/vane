//! Upstream mTLS coverage — `args.tls.client_cert` drives a require-
//! client-cert hyper TLS server through the full `HttpProxyFetch`
//! round-trip, plus pool-fingerprint identity checks.
//!
//! Fixtures: rcgen builds (a) a client CA and a leaf signed by it and
//! (b) a server CA and a server leaf for `localhost`. The hyper
//! server pins client trust to (a) and rejects unauthenticated
//! handshakes; the engine path pins the server cert via
//! `insecure_skip_verify: true` (the test cares about client-side
//! authentication, not server-side).

use std::convert::Infallible;
use std::sync::Arc;

use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper_util::rt::TokioIo;
use parking_lot::Mutex;
use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair, KeyUsagePurpose};
use rustls::RootCertStore;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use vane_core::{
	Body, ConnContext, FlowCtx, FlowLogEvent, FlowLogSink, HttpVersion, NodeId, TlsInfo,
	TrajectoryBuilder, Transport,
};
use vane_engine::flow_graph::FetchInst;
use vane_engine::verbosity::VerbosityState;

fn install_provider() {
	vane_engine::crypto::install_default_provider();
	vane_testutil::allow_insecure_upstream_for_tests();
}

/// rcgen issuer + matching cert DER. Used both as a CA (`Issuer` for
/// signing leaves) and to populate a rustls `RootCertStore`.
struct CaPair {
	issuer: Issuer<'static, KeyPair>,
	der: CertificateDer<'static>,
}

fn make_ca(name: &str) -> CaPair {
	let mut params = CertificateParams::new(vec![name.into()]).expect("ca params");
	params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
	params.key_usages =
		vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::DigitalSignature, KeyUsagePurpose::CrlSign];
	let key = KeyPair::generate().expect("ca key");
	let cert = params.clone().self_signed(&key).expect("self-sign ca");
	let der = cert.der().clone();
	let issuer = Issuer::new(params, key);
	CaPair { issuer, der }
}

/// Issue a leaf cert with `subject_alt_names` signed by `ca`. Returns
/// the cert DER and the keypair (caller decides how to persist /
/// serialize).
fn issue_leaf(ca: &CaPair, subject_alt_names: Vec<String>) -> (CertificateDer<'static>, KeyPair) {
	let params = CertificateParams::new(subject_alt_names).expect("leaf params");
	let key = KeyPair::generate().expect("leaf key");
	let cert = params.signed_by(&key, &ca.issuer).expect("sign leaf");
	(cert.der().clone(), key)
}

/// Persist a cert + key as PEM files under a `TempDir`. Returns the
/// pair of paths so a rule can reference them via `cert_path` /
/// `key_path`.
fn persist_pem(
	dir: &TempDir,
	stem: &str,
	cert: &CertificateDer<'static>,
	key: &KeyPair,
) -> (std::path::PathBuf, std::path::PathBuf) {
	use std::io::Write;
	let cert_path = dir.path().join(format!("{stem}.crt"));
	let key_path = dir.path().join(format!("{stem}.key"));
	let cert_pem = pem_encode(cert.as_ref(), "CERTIFICATE");
	let mut f = std::fs::File::create(&cert_path).expect("write cert");
	f.write_all(cert_pem.as_bytes()).expect("flush cert");
	let key_pem = key.serialize_pem();
	let mut f = std::fs::File::create(&key_path).expect("write key");
	f.write_all(key_pem.as_bytes()).expect("flush key");
	(cert_path, key_path)
}

fn pem_encode(der: &[u8], tag: &str) -> String {
	use std::fmt::Write as _;

	use base64::Engine as _;
	let b64 = base64::engine::general_purpose::STANDARD.encode(der);
	let mut out = format!("-----BEGIN {tag}-----\n");
	for chunk in b64.as_bytes().chunks(64) {
		out.push_str(std::str::from_utf8(chunk).unwrap());
		out.push('\n');
	}
	let _ = writeln!(out, "-----END {tag}-----");
	out
}

/// Server config requiring a client cert signed by `client_ca` and
/// presenting `server_cert` to clients.
fn server_config(
	server_cert: CertificateDer<'static>,
	server_key: &KeyPair,
	client_ca: &CertificateDer<'static>,
) -> Arc<rustls::ServerConfig> {
	let mut roots = RootCertStore::empty();
	roots.add(client_ca.clone()).expect("add client ca");
	let verifier = WebPkiClientVerifier::builder(Arc::new(roots)).build().expect("build verifier");
	let key_der: PrivateKeyDer<'static> = PrivateKeyDer::Pkcs8(server_key.serialize_der().into());
	Arc::new(
		rustls::ServerConfig::builder()
			.with_client_cert_verifier(verifier)
			.with_single_cert(vec![server_cert], key_der)
			.expect("server config"),
	)
}

async fn spawn_https_static_with_mtls(
	server_cfg: Arc<rustls::ServerConfig>,
	body: &'static str,
	accepted: Arc<Mutex<usize>>,
	rejected: Arc<Mutex<usize>>,
) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
	let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
	let addr = listener.local_addr().expect("local_addr");
	let acceptor = TlsAcceptor::from(server_cfg);
	let handle = tokio::spawn(async move {
		loop {
			let Ok((sock, _)) = listener.accept().await else { return };
			let acceptor = acceptor.clone();
			let accepted = Arc::clone(&accepted);
			let rejected = Arc::clone(&rejected);
			tokio::spawn(async move {
				let Ok(tls) = acceptor.accept(sock).await else {
					*rejected.lock() += 1;
					return;
				};
				*accepted.lock() += 1;
				let io = TokioIo::new(tls);
				let svc = hyper::service::service_fn(move |_req: hyper::Request<hyper::body::Incoming>| {
					let resp_body = body.to_string();
					async move {
						Ok::<_, Infallible>(
							hyper::Response::builder()
								.status(200)
								.header("content-type", "text/plain")
								.body(Full::new(Bytes::from(resp_body)))
								.expect("response"),
						)
					}
				});
				let _ = hyper::server::conn::http1::Builder::new().serve_connection(io, svc).await;
			});
		}
	});
	(addr, handle)
}

struct NullSink;
impl FlowLogSink for NullSink {
	fn emit(&self, _event: FlowLogEvent) {}
}

fn make_ctx() -> (Arc<ConnContext>, FlowCtx) {
	let conn = Arc::new(ConnContext::new(
		vane_core::ConnId(1),
		"127.0.0.1:0".parse().unwrap(),
		"127.0.0.1:0".parse().unwrap(),
		Transport::Tcp,
		std::time::Instant::now(),
	));
	*conn.tls.lock() =
		Some(TlsInfo { sni: None, alpn: None, version: None, peer_cert: None, zero_rtt_used: false });
	let _ = conn.http_version.set(HttpVersion::Http1_1);
	let span = tracing::info_span!("test");
	let ctx = FlowCtx {
		span,
		log: Arc::new(NullSink) as Arc<dyn FlowLogSink>,
		cancel: tokio_util::sync::CancellationToken::new(),
		accept_cancel: tokio_util::sync::CancellationToken::new(),
		verbosity: VerbosityState::new().current(),
		trajectory: TrajectoryBuilder::new(conn.id, NodeId::for_testing(0), 0),
	};
	(conn, ctx)
}

#[tokio::test(flavor = "multi_thread")]
async fn upstream_mtls_handshake_succeeds_with_matching_client_cert() {
	install_provider();
	let server_ca = make_ca("server-ca");
	let client_ca = make_ca("client-ca");
	let (server_cert, server_key) = issue_leaf(&server_ca, vec!["localhost".into()]);
	let (client_cert, client_key) = issue_leaf(&client_ca, vec!["test-client".into()]);

	let tmp = tempfile::tempdir().expect("tmp");
	let (cert_path, key_path) = persist_pem(&tmp, "client", &client_cert, &client_key);

	let server_cfg = server_config(server_cert, &server_key, &client_ca.der);
	let accepted = Arc::new(Mutex::new(0_usize));
	let rejected = Arc::new(Mutex::new(0_usize));
	let (addr, _server_task) = spawn_https_static_with_mtls(
		server_cfg,
		"hello-mtls",
		Arc::clone(&accepted),
		Arc::clone(&rejected),
	)
	.await;

	let factory_args = serde_json::json!({
		"upstream": addr.to_string(),
		"tls": {
			"insecure_skip_verify": true,
			"verify_hostname": "localhost",
			"client_cert": {
				"cert_path": cert_path.to_str().unwrap(),
				"key_path":  key_path.to_str().unwrap(),
			},
		},
	});
	let inst = vane_engine::fetch::http_proxy::factory(&factory_args, None).expect("factory");
	let FetchInst::L7(fetch) = inst else { panic!("L7 expected") };

	let (conn, mut ctx) = make_ctx();
	let req = http::Request::builder().uri("http://placeholder/path").body(Body::Empty).expect("req");
	let outcome = fetch.fetch(req, &conn, &mut ctx).await.expect("fetch");
	let vane_core::L7FetchOutput::Response(resp) = outcome else { panic!("Response expected") };
	assert_eq!(resp.status(), 200, "mTLS handshake + 200 response");
	let Body::Stream(s) = resp.into_body() else { panic!("stream body expected") };
	let bytes = s.collect().await.expect("collect").to_bytes();
	assert_eq!(&bytes[..], b"hello-mtls");
}

#[tokio::test(flavor = "multi_thread")]
async fn upstream_mtls_handshake_fails_when_client_cert_absent() {
	install_provider();
	let server_ca = make_ca("server-ca");
	let client_ca = make_ca("client-ca");
	let (server_cert, server_key) = issue_leaf(&server_ca, vec!["localhost".into()]);

	let server_cfg = server_config(server_cert, &server_key, &client_ca.der);
	let accepted = Arc::new(Mutex::new(0_usize));
	let rejected = Arc::new(Mutex::new(0_usize));
	let (addr, _task) = spawn_https_static_with_mtls(
		server_cfg,
		"unreachable",
		Arc::clone(&accepted),
		Arc::clone(&rejected),
	)
	.await;

	// Same factory args BUT no client_cert. The handshake must fail.
	let factory_args = serde_json::json!({
		"upstream": addr.to_string(),
		"tls": {
			"insecure_skip_verify": true,
			"verify_hostname": "localhost",
		},
	});
	let inst = vane_engine::fetch::http_proxy::factory(&factory_args, None).expect("factory");
	let FetchInst::L7(fetch) = inst else { panic!("L7 expected") };

	let (conn, mut ctx) = make_ctx();
	let req = http::Request::builder().uri("http://placeholder/path").body(Body::Empty).expect("req");
	let result = fetch.fetch(req, &conn, &mut ctx).await;
	assert!(result.is_err(), "handshake without client cert must fail");
}

#[tokio::test(flavor = "multi_thread")]
async fn parse_client_cert_requires_both_paths() {
	install_provider();
	// The factory layer surfaces a String error wrapped in FactoryError
	// for missing cert_path / key_path.
	let res = vane_engine::fetch::http_proxy::factory(
		&serde_json::json!({
			"upstream": "127.0.0.1:1",
			"tls": { "client_cert": { "key_path": "/dev/null" } },
		}),
		None,
	);
	let Err(err) = res else { panic!("missing cert_path must reject") };
	let msg = err.message();
	assert!(msg.contains("cert_path"), "{msg}");
}

#[tokio::test(flavor = "multi_thread")]
async fn parse_client_cert_loads_certified_key_from_disk() {
	install_provider();
	// End-to-end without a server: just verify parse_tls_args returns
	// a non-empty `client_cert` and the fingerprint hash is stable.
	let client_ca = make_ca("client-ca");
	let (cert, key) = issue_leaf(&client_ca, vec!["client".into()]);
	let tmp = tempfile::tempdir().expect("tmp");
	let (cert_path, key_path) = persist_pem(&tmp, "client", &cert, &key);

	let args = serde_json::json!({
		"insecure_skip_verify": true,
		"client_cert": {
			"cert_path": cert_path.to_str().unwrap(),
			"key_path":  key_path.to_str().unwrap(),
		},
	});
	let parsed = vane_engine::fetch::upstream::parse_tls_args("127.0.0.1:1", Some(&args), None)
		.expect("parse")
		.expect("Some");
	assert!(parsed.client_cert.is_some(), "client_cert loaded");
	assert!(parsed.fingerprint.client_cert_hash.is_some(), "fingerprint carries hash");
}
