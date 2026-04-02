#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use vane_engine::{
	config::{
		CertEntry, ConfigTable, FlowNode, GlobalConfig, L5Config, Layer, ListenConfig, PortConfig,
		TerminationAction,
	},
	engine::Engine,
	flow::{PluginAction, PluginRegistry, TlsClientHello, builtin::tcp_forward::TcpForward},
};
use vane_test_utils::echo::EchoServer;
use vane_transport::tcp::ProxyConfig;
use vane_transport::tls::{CertStore, parse_pem};

fn generate_self_signed() -> (Vec<u8>, Vec<u8>) {
	let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
	let cert_pem = cert.cert.pem().into_bytes();
	let key_pem = cert.signing_key.serialize_pem().into_bytes();
	(cert_pem, key_pem)
}

#[derive(Debug)]
struct NoVerify;

impl ServerCertVerifier for NoVerify {
	fn verify_server_cert(
		&self,
		_end_entity: &CertificateDer<'_>,
		_intermediates: &[CertificateDer<'_>],
		_server_name: &ServerName<'_>,
		_ocsp_response: &[u8],
		_now: UnixTime,
	) -> Result<ServerCertVerified, rustls::Error> {
		Ok(ServerCertVerified::assertion())
	}

	fn verify_tls12_signature(
		&self,
		_message: &[u8],
		_cert: &CertificateDer<'_>,
		_dss: &DigitallySignedStruct,
	) -> Result<HandshakeSignatureValid, rustls::Error> {
		Ok(HandshakeSignatureValid::assertion())
	}

	fn verify_tls13_signature(
		&self,
		_message: &[u8],
		_cert: &CertificateDer<'_>,
		_dss: &DigitallySignedStruct,
	) -> Result<HandshakeSignatureValid, rustls::Error> {
		Ok(HandshakeSignatureValid::assertion())
	}

	fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
		rustls::crypto::ring::default_provider().signature_verification_algorithms.supported_schemes()
	}
}

fn build_test_client_config() -> Arc<ClientConfig> {
	let provider = Arc::new(rustls::crypto::ring::default_provider());
	let config = ClientConfig::builder_with_provider(provider)
		.with_safe_default_protocol_versions()
		.unwrap()
		.dangerous()
		.with_custom_certificate_verifier(Arc::new(NoVerify))
		.with_no_client_auth();
	Arc::new(config)
}

/// L4: tls.clienthello -> branch "localhost" -> tcp.forward with Upgrade(L5)
/// L5: tcp.forward to echo
fn build_clienthello_test_setup(
	echo_addr: std::net::SocketAddr,
) -> (ConfigTable, PluginRegistry, CertStore) {
	let forward_params = serde_json::json!({
		"ip": echo_addr.ip().to_string(),
		"port": echo_addr.port(),
	});

	// L4: tls.clienthello routes by SNI
	let localhost_branch = FlowNode {
		plugin: "tcp.forward".to_owned(),
		params: forward_params.clone(),
		branches: HashMap::new(),
		termination: Some(TerminationAction::Upgrade { target_layer: Layer::L5 }),
	};

	let default_branch = FlowNode {
		plugin: "tcp.forward".to_owned(),
		params: forward_params.clone(),
		branches: HashMap::new(),
		termination: Some(TerminationAction::Upgrade { target_layer: Layer::L5 }),
	};

	let l4 = FlowNode {
		plugin: "tls.clienthello".to_owned(),
		params: serde_json::Value::Null,
		branches: HashMap::from([
			("localhost".to_owned(), localhost_branch),
			("_default".to_owned(), default_branch),
		]),
		termination: None,
	};

	let l5 = L5Config {
		default_cert: "default".to_owned(),
		alpn: vec![],
		flow: FlowNode {
			plugin: "tcp.forward".to_owned(),
			params: forward_params,
			branches: HashMap::new(),
			termination: None,
		},
	};

	let (cert_pem, key_pem) = generate_self_signed();

	let config = ConfigTable {
		ports: HashMap::from([(
			0,
			PortConfig { listen: ListenConfig::default(), l4, l5: Some(l5), l7: None },
		)]),
		global: GlobalConfig::default(),
		certs: HashMap::from([(
			"default".to_owned(),
			CertEntry::Pem {
				cert_pem: String::from_utf8(cert_pem.clone()).unwrap(),
				key_pem: String::from_utf8(key_pem.clone()).unwrap(),
			},
		)]),
	};

	let registry = PluginRegistry::new()
		.register("tls.clienthello", PluginAction::Middleware(Box::new(TlsClientHello)))
		.register(
			"tcp.forward",
			PluginAction::Terminator(Box::new(TcpForward { proxy_config: ProxyConfig::default() })),
		);

	let loaded = parse_pem(&cert_pem, &key_pem).unwrap();
	let mut cert_store = CertStore::new();
	cert_store.insert("default", loaded);

	(config, registry, cert_store)
}

/// SNI-based routing via tls.clienthello -> TLS upgrade -> echo roundtrip.
#[tokio::test]
async fn clienthello_sni_routes_and_upgrades() {
	let echo = EchoServer::start().await;
	let (config, registry, cert_store) = build_clienthello_test_setup(echo.addr());

	let mut engine = Engine::new(config, registry, cert_store).unwrap();
	engine.start().await.unwrap();
	let listen_addr = engine.listeners()[0].local_addr();

	let client_config = build_test_client_config();
	let connector = TlsConnector::from(client_config);

	let tcp = TcpStream::connect(listen_addr).await.unwrap();
	let server_name = ServerName::try_from("localhost").unwrap();
	let mut tls_stream = connector.connect(server_name, tcp).await.unwrap();

	let payload = b"hello via clienthello routing";
	tls_stream.write_all(payload).await.unwrap();

	let mut response = vec![0u8; payload.len()];
	tls_stream.read_exact(&mut response).await.unwrap();

	assert_eq!(response, payload);

	engine.shutdown();
	engine.join().await;
}

/// SNI="unknown.com" -> middleware returns branch "unknown.com" -> no matching branch in config
/// -> `BranchNotFound` error -> connection closed gracefully
#[tokio::test]
async fn clienthello_sni_no_matching_branch() {
	let echo = EchoServer::start().await;
	let (config, registry, cert_store) = build_clienthello_test_setup(echo.addr());

	let mut engine = Engine::new(config, registry, cert_store).unwrap();
	engine.start().await.unwrap();
	let listen_addr = engine.listeners()[0].local_addr();

	let client_config = build_test_client_config();
	let connector = TlsConnector::from(client_config);

	let tcp = TcpStream::connect(listen_addr).await.unwrap();
	// "unknown.com" has no matching branch in the config (only "localhost" and "_default")
	let server_name = ServerName::try_from("unknown.com").unwrap();
	let result = connector.connect(server_name, tcp).await;

	// The server closes the connection due to BranchNotFound error,
	// so the TLS handshake never completes -- client sees a connection error
	assert!(result.is_err(), "connection should fail when SNI has no matching branch");

	engine.shutdown();
	engine.join().await;
}
