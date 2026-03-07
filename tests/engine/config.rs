#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use vane_engine::{
	config::{CertEntry, ConfigTable, FlowNode, GlobalConfig, L5Config, ListenConfig, PortConfig},
	engine::{Engine, EngineError},
	flow::{PluginAction, PluginRegistry, builtin::tcp_forward::TcpForward},
};
use vane_test_utils::echo::EchoServer;
use vane_transport::tcp::ProxyConfig;
use vane_transport::tls::CertStore;

#[test]
fn engine_rejects_invalid_config() {
	let node = FlowNode {
		plugin: "nonexistent.plugin".to_owned(),
		params: serde_json::Value::default(),
		branches: HashMap::new(),
		termination: None,
	};

	let config = ConfigTable {
		ports: HashMap::from([(
			80,
			PortConfig { listen: ListenConfig::default(), l4: node, l5: None, l7: None },
		)]),
		global: GlobalConfig::default(),
		certs: HashMap::new(),
	};

	let registry = PluginRegistry::new();
	let result = Engine::new(config, registry, CertStore::new());
	assert!(matches!(result, Err(EngineError::ConfigInvalid(_))));
}

#[tokio::test]
async fn update_config_hot_reload() {
	let echo_a = EchoServer::start().await;
	let echo_b = EchoServer::start().await;

	let make_node = |addr: std::net::SocketAddr| FlowNode {
		plugin: "tcp.forward".to_owned(),
		params: serde_json::json!({
			"ip": addr.ip().to_string(),
			"port": addr.port(),
		}),
		branches: HashMap::new(),
		termination: None,
	};

	let make_config = |addr: std::net::SocketAddr| ConfigTable {
		ports: HashMap::from([(
			0,
			PortConfig { listen: ListenConfig::default(), l4: make_node(addr), l5: None, l7: None },
		)]),
		global: GlobalConfig::default(),
		certs: HashMap::new(),
	};

	let registry = PluginRegistry::new().register(
		"tcp.forward",
		PluginAction::Terminator(Box::new(TcpForward { proxy_config: ProxyConfig::default() })),
	);

	let mut engine = Engine::new(make_config(echo_a.addr()), registry, CertStore::new()).unwrap();
	engine.start().await.unwrap();
	let listen_addr = engine.listeners()[0].local_addr();

	// Verify initial config (forwards to echo_a)
	{
		let mut client = TcpStream::connect(listen_addr).await.unwrap();
		client.write_all(b"before reload").await.unwrap();
		let mut buf = Vec::new();
		client.read_to_end(&mut buf).await.unwrap();
		assert_eq!(buf, b"before reload");
	}

	// Hot-reload: point at echo_b
	engine.update_config(make_config(echo_b.addr())).unwrap();

	{
		let mut client = TcpStream::connect(listen_addr).await.unwrap();
		client.write_all(b"after reload").await.unwrap();
		let mut buf = Vec::new();
		client.read_to_end(&mut buf).await.unwrap();
		assert_eq!(buf, b"after reload");
	}

	engine.shutdown();
	engine.join().await;
}

#[tokio::test]
async fn connection_limit_rejects() {
	// Upstream that holds connections open indefinitely
	let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let upstream_addr = upstream.local_addr().unwrap();
	let upstream_handle = tokio::spawn(async move {
		let (_stream, _) = upstream.accept().await.unwrap();
		tokio::time::sleep(Duration::from_secs(30)).await;
	});

	let node = FlowNode {
		plugin: "tcp.forward".to_owned(),
		params: serde_json::json!({
			"ip": upstream_addr.ip().to_string(),
			"port": upstream_addr.port(),
		}),
		branches: HashMap::new(),
		termination: None,
	};

	let config = ConfigTable {
		ports: HashMap::from([(
			0,
			PortConfig { listen: ListenConfig::default(), l4: node, l5: None, l7: None },
		)]),
		global: GlobalConfig { max_connections_per_ip: 1, ..Default::default() },
		certs: HashMap::new(),
	};

	let registry = PluginRegistry::new().register(
		"tcp.forward",
		PluginAction::Terminator(Box::new(TcpForward { proxy_config: ProxyConfig::default() })),
	);

	let mut engine = Engine::new(config, registry, CertStore::new()).unwrap();
	engine.start().await.unwrap();
	let listen_addr = engine.listeners()[0].local_addr();

	// Hold first connection open through the proxy
	let mut first = TcpStream::connect(listen_addr).await.unwrap();
	first.write_all(b"hold").await.unwrap();
	tokio::time::sleep(Duration::from_millis(100)).await;

	// Second connection from same IP — guard acquisition fails, stream dropped
	let mut second = TcpStream::connect(listen_addr).await.unwrap();
	let mut buf = vec![0u8; 64];
	let result = tokio::time::timeout(Duration::from_secs(2), second.read(&mut buf)).await;

	match result {
		Ok(Ok(0) | Err(_)) => {} // EOF or connection reset
		other => panic!("expected rejection (EOF or error), got {other:?}"),
	}

	drop(first);
	engine.shutdown();
	engine.join().await;
	upstream_handle.abort();
}

/// Engine rejects config that references L5 with upgrade but cert store is empty.
#[test]
fn tls_build_fails_with_empty_cert_store() {
	use vane_engine::config::{Layer, TerminationAction};

	let l4_node = FlowNode {
		plugin: "tcp.forward".to_owned(),
		params: serde_json::json!({"ip": "127.0.0.1", "port": 1}),
		branches: HashMap::new(),
		termination: Some(TerminationAction::Upgrade { target_layer: Layer::L5 }),
	};

	let l5 = L5Config {
		default_cert: "test".to_owned(),
		alpn: vec![],
		flow: FlowNode {
			plugin: "tcp.forward".to_owned(),
			params: serde_json::json!({"ip": "127.0.0.1", "port": 1}),
			branches: HashMap::new(),
			termination: None,
		},
	};

	let config = ConfigTable {
		ports: HashMap::from([(
			443,
			PortConfig { listen: ListenConfig::default(), l4: l4_node, l5: Some(l5), l7: None },
		)]),
		global: GlobalConfig::default(),
		certs: HashMap::from([(
			"test".to_owned(),
			CertEntry::Pem { cert_pem: "CERT".to_owned(), key_pem: "KEY".to_owned() },
		)]),
	};

	let registry = PluginRegistry::new().register(
		"tcp.forward",
		PluginAction::Terminator(Box::new(TcpForward { proxy_config: ProxyConfig::default() })),
	);

	// Empty cert store — TLS config build should fail
	let result = Engine::new(config, registry, CertStore::new());
	assert!(matches!(result, Err(EngineError::TlsBuildFailed { .. })));
}
