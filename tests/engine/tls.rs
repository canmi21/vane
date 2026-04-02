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
	flow::{PluginAction, PluginRegistry, ProtocolDetect, builtin::tcp_forward::TcpForward},
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

/// Build engine config for protocol detection with TLS upgrade.
///
/// L4: protocol.detect
///   - "tls" branch: tcp.forward with Upgrade(L5)
///   - "unknown" branch: tcp.forward (passthrough)
///
/// L5: tcp.forward to echo
fn build_tls_test_setup(
	echo_addr: std::net::SocketAddr,
) -> (ConfigTable, PluginRegistry, CertStore) {
	let forward_params = serde_json::json!({
		"ip": echo_addr.ip().to_string(),
		"port": echo_addr.port(),
	});

	// L4 TLS branch: forward with Upgrade(L5)
	let tls_branch = FlowNode {
		plugin: "tcp.forward".to_owned(),
		params: forward_params.clone(),
		branches: HashMap::new(),
		termination: Some(TerminationAction::Upgrade { target_layer: Layer::L5 }),
	};

	// L4 passthrough branch: forward directly
	let unknown_branch = FlowNode {
		plugin: "tcp.forward".to_owned(),
		params: forward_params.clone(),
		branches: HashMap::new(),
		termination: None,
	};

	let l4 = FlowNode {
		plugin: "protocol.detect".to_owned(),
		params: serde_json::Value::Null,
		branches: HashMap::from([
			("tls".to_owned(), tls_branch),
			("unknown".to_owned(), unknown_branch),
			(
				"http".to_owned(),
				FlowNode {
					plugin: "tcp.forward".to_owned(),
					params: forward_params.clone(),
					branches: HashMap::new(),
					termination: None,
				},
			),
		]),
		termination: None,
	};

	// L5 flow: forward decrypted traffic to echo
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
		.register(
			"protocol.detect",
			PluginAction::Middleware(Box::new(ProtocolDetect::with_defaults())),
		)
		.register(
			"tcp.forward",
			PluginAction::Terminator(Box::new(TcpForward { proxy_config: ProxyConfig::default() })),
		);

	let loaded = parse_pem(&cert_pem, &key_pem).unwrap();
	let mut cert_store = CertStore::new();
	cert_store.insert("default", loaded);

	(config, registry, cert_store)
}

/// L4 detects TLS -> Upgrade(L5) -> TLS handshake -> L5 forwards to echo -> roundtrip.
#[tokio::test]
async fn tls_upgrade_forwards_to_echo() {
	let echo = EchoServer::start().await;
	let (config, registry, cert_store) = build_tls_test_setup(echo.addr());

	let mut engine = Engine::new(config, registry, cert_store).unwrap();
	engine.start().await.unwrap();
	let listen_addr = engine.listeners()[0].local_addr();

	let client_config = build_test_client_config();
	let connector = TlsConnector::from(client_config);

	let tcp = TcpStream::connect(listen_addr).await.unwrap();
	let server_name = ServerName::try_from("localhost").unwrap();
	let mut tls_stream = connector.connect(server_name, tcp).await.unwrap();

	let payload = b"hello through tls";
	tls_stream.write_all(payload).await.unwrap();

	// Read the exact echo response. Don't call shutdown() before reading —
	// the echo server echoes and closes immediately, so proxy_tcp returns
	// and the server's TLS stream is dropped without close_notify.
	let mut response = vec![0u8; payload.len()];
	tls_stream.read_exact(&mut response).await.unwrap();

	assert_eq!(response, payload);

	engine.shutdown();
	engine.join().await;
}

/// TLS-like bytes (0x16 prefix) pass protocol detection as "tls" but fail TLS handshake.
/// Connection should be closed gracefully without panic.
#[tokio::test]
async fn tls_like_bytes_handshake_failure_closes() {
	let echo = EchoServer::start().await;
	let (config, registry, cert_store) = build_tls_test_setup(echo.addr());

	let mut engine = Engine::new(config, registry, cert_store).unwrap();
	engine.start().await.unwrap();
	let listen_addr = engine.listeners()[0].local_addr();

	let mut client = TcpStream::connect(listen_addr).await.unwrap();
	// Send bytes that look like a TLS record header (0x16 = handshake)
	// but contain garbage -- passes ProtocolDetect as "tls" branch,
	// triggers Upgrade(L5), then accept_tls fails on handshake
	let fake_tls = [
		0x16, 0x03, 0x01, 0x00, 0x05, // record header
		0x01, 0x00, 0x00, 0x01, 0x00, // garbage handshake data
	];
	client.write_all(&fake_tls).await.unwrap();

	// Server closes after handshake failure (rustls may send a TLS alert first)
	let mut buf = Vec::new();
	let _ = client.read_to_end(&mut buf).await.unwrap();
	// Connection terminated — no echo of application data

	engine.shutdown();
	engine.join().await;
}

/// Non-TLS client on the same port goes through the "unknown" branch directly.
#[tokio::test]
async fn non_tls_passthrough() {
	let echo = EchoServer::start().await;
	let (config, registry, cert_store) = build_tls_test_setup(echo.addr());

	let mut engine = Engine::new(config, registry, cert_store).unwrap();
	engine.start().await.unwrap();
	let listen_addr = engine.listeners()[0].local_addr();

	let mut client = TcpStream::connect(listen_addr).await.unwrap();
	// Random non-TLS bytes → "unknown" branch → tcp.forward passthrough
	let data = [0xDE, 0xAD, 0xBE, 0xEF];
	client.write_all(&data).await.unwrap();

	let mut response = Vec::new();
	client.read_to_end(&mut response).await.unwrap();

	assert_eq!(response, data);

	engine.shutdown();
	engine.join().await;
}
