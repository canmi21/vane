#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use vane_engine::{
	config::{ConfigTable, GlobalConfig, ListenConfig, PortConfig, TargetAddr},
	engine::{Engine, EngineError},
};
use vane_test_utils::echo::EchoServer;

#[test]
fn engine_rejects_invalid_config() {
	let config = ConfigTable {
		ports: HashMap::from([(
			80,
			PortConfig {
				listen: ListenConfig::default(),
				target: TargetAddr { ip: "not-an-ip".to_owned(), port: 8080 },
			},
		)]),
		global: GlobalConfig::default(),
	};

	let result = Engine::new(config);
	assert!(matches!(result, Err(EngineError::ConfigInvalid(_))));
}

#[tokio::test]
async fn update_config_hot_reload() {
	let echo_a = EchoServer::start().await;
	let echo_b = EchoServer::start().await;

	let make_config = |addr: std::net::SocketAddr| ConfigTable {
		ports: HashMap::from([(
			0,
			PortConfig {
				listen: ListenConfig::default(),
				target: TargetAddr { ip: addr.ip().to_string(), port: addr.port() },
			},
		)]),
		global: GlobalConfig::default(),
	};

	let engine = Engine::new(make_config(echo_a.addr())).unwrap();
	engine.start().await.unwrap();
	let listen_addr = engine.listener_addr(0).unwrap();

	// Verify initial config (forwards to echo_a)
	{
		let mut client = TcpStream::connect(listen_addr).await.unwrap();
		client.write_all(b"before reload").await.unwrap();
		let mut buf = Vec::new();
		client.read_to_end(&mut buf).await.unwrap();
		assert_eq!(buf, b"before reload");
	}

	// Hot-reload: point at echo_b
	engine.update_config(make_config(echo_b.addr())).await.unwrap();

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
	let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let upstream_addr = upstream.local_addr().unwrap();
	let upstream_handle = tokio::spawn(async move {
		let (_stream, _) = upstream.accept().await.unwrap();
		tokio::time::sleep(Duration::from_secs(30)).await;
	});

	let config = ConfigTable {
		ports: HashMap::from([(
			0,
			PortConfig {
				listen: ListenConfig::default(),
				target: TargetAddr { ip: upstream_addr.ip().to_string(), port: upstream_addr.port() },
			},
		)]),
		global: GlobalConfig { max_connections_per_ip: 1, ..Default::default() },
	};

	let engine = Engine::new(config).unwrap();
	engine.start().await.unwrap();
	let listen_addr = engine.listener_addr(0).unwrap();

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
