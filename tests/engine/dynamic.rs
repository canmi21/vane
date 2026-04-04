#![allow(clippy::unwrap_used)]

use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use vane_engine::{
	config::{ConfigTable, GlobalConfig, ListenerRule, Protocol, TargetAddr},
	engine::Engine,
};
use vane_test_utils::echo::EchoServer;

fn tcp_rule(port: &str) -> ListenerRule {
	ListenerRule { bind: "0.0.0.0".to_owned(), port: port.to_owned(), protocol: Protocol::Tcp }
}

fn config_with_target(listeners: Vec<ListenerRule>, addr: std::net::SocketAddr) -> ConfigTable {
	ConfigTable {
		listeners,
		target: Some(TargetAddr { ip: addr.ip().to_string(), port: addr.port() }),
		global: GlobalConfig::default(),
	}
}

const BIND_ANY: &str = "0.0.0.0:0";

fn key() -> SocketAddr {
	BIND_ANY.parse().unwrap()
}

/// `start` launches a listener that accepts TCP connections.
#[tokio::test]
async fn start_then_connect() {
	let echo = EchoServer::start().await;
	let config = config_with_target(vec![tcp_rule("0")], echo.addr());
	let engine = Engine::new(config).unwrap();

	engine.start().await.unwrap();
	let listen_addr = engine.listener_addr(key()).unwrap();

	let mut client = TcpStream::connect(listen_addr).await.unwrap();
	client.write_all(b"dynamic start").await.unwrap();
	let mut buf = Vec::new();
	client.read_to_end(&mut buf).await.unwrap();
	assert_eq!(buf, b"dynamic start");

	engine.shutdown();
	engine.join().await;
}

/// Removing a listener rule stops it so the port refuses new connections.
#[tokio::test]
async fn update_config_removes_listener() {
	let echo = EchoServer::start().await;
	let config = config_with_target(vec![tcp_rule("0")], echo.addr());
	let engine = Engine::new(config).unwrap();
	engine.start().await.unwrap();

	let listen_addr = engine.listener_addr(key()).unwrap();

	{
		let mut client = TcpStream::connect(listen_addr).await.unwrap();
		client.write_all(b"before remove").await.unwrap();
		let mut buf = Vec::new();
		client.read_to_end(&mut buf).await.unwrap();
		assert_eq!(buf, b"before remove");
	}

	// Remove all listeners
	engine
		.update_config(ConfigTable {
			listeners: vec![],
			target: Some(TargetAddr { ip: echo.addr().ip().to_string(), port: echo.addr().port() }),
			global: GlobalConfig::default(),
		})
		.await
		.unwrap();
	assert!(engine.listener_addr(key()).is_none());

	tokio::time::sleep(Duration::from_millis(50)).await;

	let result =
		tokio::time::timeout(Duration::from_millis(200), TcpStream::connect(listen_addr)).await;
	assert!(
		result.is_err() || result.unwrap().is_err(),
		"connection should fail after listener removed"
	);

	engine.join().await;
}

/// Adding a listener rule via `update_config` starts it automatically.
#[tokio::test]
async fn update_config_adds_listener() {
	let echo = EchoServer::start().await;

	// Start with no listeners
	let engine = Engine::new(ConfigTable {
		listeners: vec![],
		target: Some(TargetAddr { ip: echo.addr().ip().to_string(), port: echo.addr().port() }),
		global: GlobalConfig::default(),
	})
	.unwrap();
	engine.start().await.unwrap();
	assert!(engine.listener_addrs().is_empty());

	// Add a listener
	let new_config = config_with_target(vec![tcp_rule("0")], echo.addr());
	engine.update_config(new_config).await.unwrap();

	let listen_addr = engine.listener_addr(key()).unwrap();

	let mut client = TcpStream::connect(listen_addr).await.unwrap();
	client.write_all(b"added listener").await.unwrap();
	let mut buf = Vec::new();
	client.read_to_end(&mut buf).await.unwrap();
	assert_eq!(buf, b"added listener");

	engine.shutdown();
	engine.join().await;
}

/// Changing only the target hot-reloads without restarting listeners.
#[tokio::test]
async fn update_config_hot_reload_target() {
	let echo_a = EchoServer::start().await;
	let echo_b = EchoServer::start().await;

	let engine = Engine::new(config_with_target(vec![tcp_rule("0")], echo_a.addr())).unwrap();
	engine.start().await.unwrap();
	let listen_addr = engine.listener_addr(key()).unwrap();

	{
		let mut client = TcpStream::connect(listen_addr).await.unwrap();
		client.write_all(b"echo_a").await.unwrap();
		let mut buf = Vec::new();
		client.read_to_end(&mut buf).await.unwrap();
		assert_eq!(buf, b"echo_a");
	}

	// Same listeners, different target
	engine.update_config(config_with_target(vec![tcp_rule("0")], echo_b.addr())).await.unwrap();

	// Listener stays on same address
	assert_eq!(engine.listener_addr(key()).unwrap(), listen_addr);

	{
		let mut client = TcpStream::connect(listen_addr).await.unwrap();
		client.write_all(b"echo_b").await.unwrap();
		let mut buf = Vec::new();
		client.read_to_end(&mut buf).await.unwrap();
		assert_eq!(buf, b"echo_b");
	}

	engine.shutdown();
	engine.join().await;
}
