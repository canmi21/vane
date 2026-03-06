#![allow(clippy::unwrap_used)]

use std::collections::HashMap;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use vane_engine::{
	config::{ConfigTable, FlowNode, GlobalConfig, ListenConfig, PortConfig},
	engine::Engine,
	flow::{PluginAction, PluginRegistry, builtin::tcp_forward::TcpForward},
};
use vane_test_utils::echo::EchoServer;
use vane_transport::tcp::ProxyConfig;

#[tokio::test]
async fn test_echo_forward() {
	let echo = EchoServer::start().await;
	let echo_addr = echo.addr();

	let node = FlowNode {
		plugin: "tcp.forward".to_owned(),
		params: serde_json::json!({
				"ip": echo_addr.ip().to_string(),
				"port": echo_addr.port(),
		}),
		branches: HashMap::new(),
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
	let registry = PluginRegistry::new().register(
		"tcp.forward",
		PluginAction::Terminator(Box::new(TcpForward { proxy_config: ProxyConfig::default() })),
	);

	let mut engine = Engine::new(config, registry).unwrap();
	engine.start().await.unwrap();

	let listen_addr = engine.listeners()[0].local_addr();

	let mut client = TcpStream::connect(listen_addr).await.unwrap();
	client.write_all(b"hello vane").await.unwrap();

	let mut response = Vec::new();
	client.read_to_end(&mut response).await.unwrap();

	assert_eq!(response, b"hello vane");

	engine.shutdown();
	engine.join().await;
}
