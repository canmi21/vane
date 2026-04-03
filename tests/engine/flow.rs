#![allow(clippy::unwrap_used)]

use std::collections::HashMap;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use vane_engine::{
	config::{ConfigTable, FlowNode, GlobalConfig, ListenConfig, PortConfig},
	engine::Engine,
	flow::{
		PluginAction, PluginRegistry,
		builtin::{echo_branch::EchoBranch, tcp_forward::TcpForward},
	},
};
use vane_test_utils::echo::EchoServer;
use vane_transport::stream::ConnectionStream;
use vane_transport::tcp::ProxyConfig;
use vane_transport::tls::CertStore;

/// Multi-step flow: echo.branch middleware -> tcp.forward terminator.
#[tokio::test]
async fn test_multi_step_flow() {
	let echo = EchoServer::start().await;
	let echo_addr = echo.addr();

	let node = FlowNode {
		plugin: "echo.branch".to_owned(),
		params: serde_json::json!({"branch": "default"}),
		branches: HashMap::from([(
			"default".to_owned(),
			FlowNode {
				plugin: "tcp.forward".to_owned(),
				params: serde_json::json!({
						"ip": echo_addr.ip().to_string(),
						"port": echo_addr.port(),
				}),
				branches: HashMap::new(),
				termination: None,
			},
		)]),
		termination: None,
	};

	let config = ConfigTable {
		ports: HashMap::from([(
			0,
			PortConfig { listen: ListenConfig::default(), l4: node, l5: None, l7: None },
		)]),
		global: GlobalConfig::default(),
		certs: HashMap::new(),
	};
	let registry = PluginRegistry::new()
		.register("echo.branch", PluginAction::Middleware(Box::new(EchoBranch)))
		.register(
			"tcp.forward",
			PluginAction::Terminator(Box::new(TcpForward { proxy_config: ProxyConfig::default() })),
		);

	let engine = Engine::new(config, registry, CertStore::new()).unwrap();
	engine.start().await.unwrap();

	let listen_addr = engine.listener_addr(0).unwrap();

	let mut client = TcpStream::connect(listen_addr).await.unwrap();
	client.write_all(b"multi step").await.unwrap();

	let mut response = Vec::new();
	client.read_to_end(&mut response).await.unwrap();

	assert_eq!(response, b"multi step");

	engine.shutdown();
	engine.join().await;
}

/// Missing branch: middleware returns a branch name not present in branches map.
/// Config is valid (has a "default" branch), but at runtime echo.branch returns "nonexistent".
#[tokio::test]
async fn test_missing_branch_does_not_panic() {
	let registry = PluginRegistry::new()
		.register("echo.branch", PluginAction::Middleware(Box::new(EchoBranch)))
		.register(
			"tcp.forward",
			PluginAction::Terminator(Box::new(TcpForward { proxy_config: ProxyConfig::default() })),
		);

	// Config is valid: has "default" branch. But at runtime, echo.branch
	// with params {"branch": "nonexistent"} returns "nonexistent" which is not in the map.
	let node = FlowNode {
		plugin: "echo.branch".to_owned(),
		params: serde_json::json!({"branch": "nonexistent"}),
		branches: HashMap::from([(
			"default".to_owned(),
			FlowNode {
				plugin: "tcp.forward".to_owned(),
				params: serde_json::json!({"ip": "127.0.0.1", "port": 1}),
				branches: HashMap::new(),
				termination: None,
			},
		)]),
		termination: None,
	};

	let config = ConfigTable {
		ports: HashMap::from([(
			0,
			PortConfig { listen: ListenConfig::default(), l4: node, l5: None, l7: None },
		)]),
		global: GlobalConfig::default(),
		certs: HashMap::new(),
	};

	let engine = Engine::new(config, registry, CertStore::new()).unwrap();
	engine.start().await.unwrap();

	let listen_addr = engine.listener_addr(0).unwrap();

	// Connect; the handler should log BranchNotFound and close the connection
	let mut client = TcpStream::connect(listen_addr).await.unwrap();
	client.write_all(b"test").await.unwrap();

	let mut response = Vec::new();
	// Connection should be closed by the server side (flow error)
	let _ = client.read_to_end(&mut response).await;

	engine.shutdown();
	engine.join().await;
}

/// Timeout: a flow that takes too long is terminated by `flow_timeout`.
#[tokio::test]
async fn test_flow_timeout() {
	use std::future::Future;
	use std::net::SocketAddr;
	use std::pin::Pin;
	use std::time::Duration;
	use vane_engine::flow::Terminator;
	use vane_primitives::kv::KvStore;

	struct NeverTerminator;
	impl Terminator for NeverTerminator {
		fn execute(
			&self,
			_params: &serde_json::Value,
			_kv: &KvStore,
			_stream: ConnectionStream,
			_peer_addr: SocketAddr,
			_server_addr: SocketAddr,
		) -> Pin<Box<dyn Future<Output = Result<(), anyhow::Error>> + Send + '_>> {
			Box::pin(async {
				// Never completes
				tokio::time::sleep(Duration::from_secs(3600)).await;
				Ok(())
			})
		}
	}

	let node = FlowNode {
		plugin: "never".to_owned(),
		params: serde_json::Value::default(),
		branches: HashMap::new(),
		termination: None,
	};

	let config = ConfigTable {
		ports: HashMap::from([(
			0,
			PortConfig { listen: ListenConfig::default(), l4: node, l5: None, l7: None },
		)]),
		global: GlobalConfig { flow_timeout_ms: 100, ..Default::default() },
		certs: HashMap::new(),
	};
	let registry =
		PluginRegistry::new().register("never", PluginAction::Terminator(Box::new(NeverTerminator)));

	let engine = Engine::new(config, registry, CertStore::new()).unwrap();
	engine.start().await.unwrap();

	let listen_addr = engine.listener_addr(0).unwrap();

	let mut client = TcpStream::connect(listen_addr).await.unwrap();
	client.write_all(b"timeout test").await.unwrap();

	let mut response = Vec::new();
	// Server should drop the connection after timeout
	let _ = client.read_to_end(&mut response).await;

	engine.shutdown();
	engine.join().await;
}
