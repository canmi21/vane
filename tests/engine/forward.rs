#![allow(clippy::unwrap_used)]

use std::collections::HashMap;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use vane_engine::{
	config::{ConfigTable, GlobalConfig, ListenConfig, PortConfig, TargetAddr},
	engine::Engine,
};
use vane_test_utils::echo::EchoServer;

#[tokio::test]
async fn test_echo_forward() {
	let echo = EchoServer::start().await;
	let echo_addr = echo.addr();

	let config = ConfigTable {
		ports: HashMap::from([(
			0,
			PortConfig {
				listen: ListenConfig::default(),
				target: TargetAddr { ip: echo_addr.ip().to_string(), port: echo_addr.port() },
			},
		)]),
		global: GlobalConfig::default(),
	};

	let engine = Engine::new(config).unwrap();
	engine.start().await.unwrap();

	let listen_addr = engine.listener_addr(0).unwrap();

	let mut client = TcpStream::connect(listen_addr).await.unwrap();
	client.write_all(b"hello vane").await.unwrap();

	let mut response = Vec::new();
	client.read_to_end(&mut response).await.unwrap();

	assert_eq!(response, b"hello vane");

	engine.shutdown();
	engine.join().await;
}
