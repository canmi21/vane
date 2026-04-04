#![allow(clippy::unwrap_used)]

use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use vane_engine::{
	config::{ConfigTable, GlobalConfig, ListenerRule, Protocol, TargetAddr},
	engine::{Engine, EngineError},
};
use vane_test_utils::echo::EchoServer;

fn tcp_rule(port: &str) -> ListenerRule {
	ListenerRule { bind: "0.0.0.0".to_owned(), port: port.to_owned(), protocol: Protocol::Tcp }
}

#[test]
fn engine_rejects_invalid_config() {
	let config = ConfigTable {
		listeners: vec![ListenerRule {
			bind: "not-an-ip".to_owned(),
			port: "8080".to_owned(),
			protocol: Protocol::Tcp,
		}],
		target: Some(TargetAddr { ip: "127.0.0.1".to_owned(), port: 8080 }),
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
		listeners: vec![tcp_rule("0")],
		target: Some(TargetAddr { ip: addr.ip().to_string(), port: addr.port() }),
		global: GlobalConfig::default(),
	};

	let engine = Engine::new(make_config(echo_a.addr())).unwrap();
	engine.start().await.unwrap();
	let key: SocketAddr = "0.0.0.0:0".parse().unwrap();
	let listen_addr = engine.listener_addr(key).unwrap();

	{
		let mut client = TcpStream::connect(listen_addr).await.unwrap();
		client.write_all(b"before reload").await.unwrap();
		let mut buf = Vec::new();
		client.read_to_end(&mut buf).await.unwrap();
		assert_eq!(buf, b"before reload");
	}

	// Hot-reload: point at echo_b (same listeners, different target)
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
		listeners: vec![tcp_rule("0")],
		target: Some(TargetAddr { ip: upstream_addr.ip().to_string(), port: upstream_addr.port() }),
		global: GlobalConfig { max_connections_per_ip: 1, ..Default::default() },
	};

	let engine = Engine::new(config).unwrap();
	engine.start().await.unwrap();
	let key: SocketAddr = "0.0.0.0:0".parse().unwrap();
	let listen_addr = engine.listener_addr(key).unwrap();

	let mut first = TcpStream::connect(listen_addr).await.unwrap();
	first.write_all(b"hold").await.unwrap();
	tokio::time::sleep(Duration::from_millis(100)).await;

	let mut second = TcpStream::connect(listen_addr).await.unwrap();
	let mut buf = vec![0u8; 64];
	let result = tokio::time::timeout(Duration::from_secs(2), second.read(&mut buf)).await;

	match result {
		Ok(Ok(0) | Err(_)) => {}
		other => panic!("expected rejection (EOF or error), got {other:?}"),
	}

	drop(first);
	engine.shutdown();
	engine.join().await;
	upstream_handle.abort();
}
