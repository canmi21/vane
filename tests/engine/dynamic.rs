#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use vane_engine::{
	config::{ConfigTable, FlowNode, GlobalConfig, ListenConfig, PortConfig},
	engine::Engine,
	flow::{PluginAction, PluginRegistry, builtin::tcp_forward::TcpForward},
};
use vane_test_utils::echo::EchoServer;
use vane_transport::tcp::ProxyConfig;
use vane_transport::tls::CertStore;

fn make_registry() -> PluginRegistry {
	PluginRegistry::new().register(
		"tcp.forward",
		PluginAction::Terminator(Box::new(TcpForward { proxy_config: ProxyConfig::default() })),
	)
}

fn make_port_config(echo_addr: std::net::SocketAddr) -> PortConfig {
	PortConfig {
		listen: ListenConfig::default(),
		l4: FlowNode {
			plugin: "tcp.forward".to_owned(),
			params: serde_json::json!({
				"ip": echo_addr.ip().to_string(),
				"port": echo_addr.port(),
			}),
			branches: HashMap::new(),
			termination: None,
		},
		l5: None,
		l7: None,
	}
}

fn make_config(ports: HashMap<u16, PortConfig>) -> ConfigTable {
	ConfigTable { ports, global: GlobalConfig::default(), certs: HashMap::new() }
}

/// `start_port` launches a listener that accepts TCP connections.
#[tokio::test]
async fn start_port_then_connect() {
	let echo = EchoServer::start().await;

	let config = make_config(HashMap::from([(0, make_port_config(echo.addr()))]));
	let engine = Engine::new(config, make_registry(), CertStore::new()).unwrap();

	engine.start_port(0).await.unwrap();
	let listen_addr = engine.listener_addr(0).unwrap();

	let mut client = TcpStream::connect(listen_addr).await.unwrap();
	client.write_all(b"dynamic start").await.unwrap();
	let mut buf = Vec::new();
	client.read_to_end(&mut buf).await.unwrap();
	assert_eq!(buf, b"dynamic start");

	engine.shutdown();
	engine.join().await;
}

/// `stop_port` shuts down the listener so the port refuses new connections.
#[tokio::test]
async fn stop_port_refuses_connections() {
	let echo = EchoServer::start().await;

	let config = make_config(HashMap::from([(0, make_port_config(echo.addr()))]));
	let engine = Engine::new(config, make_registry(), CertStore::new()).unwrap();

	engine.start_port(0).await.unwrap();
	let listen_addr = engine.listener_addr(0).unwrap();

	// Verify it works first
	{
		let mut client = TcpStream::connect(listen_addr).await.unwrap();
		client.write_all(b"before stop").await.unwrap();
		let mut buf = Vec::new();
		client.read_to_end(&mut buf).await.unwrap();
		assert_eq!(buf, b"before stop");
	}

	engine.stop_port(0).unwrap();
	assert!(engine.listener_addr(0).is_none());

	// Give the listener task time to shut down
	tokio::time::sleep(Duration::from_millis(50)).await;

	// New connections should fail
	let result =
		tokio::time::timeout(Duration::from_millis(200), TcpStream::connect(listen_addr)).await;
	assert!(result.is_err() || result.unwrap().is_err(), "connection should fail after stop_port");

	engine.join().await;
}

/// `update_config` with a new port starts the listener automatically.
#[tokio::test]
async fn update_config_adds_port() {
	let echo = EchoServer::start().await;

	// Start with empty config
	let engine = Engine::new(ConfigTable::default(), make_registry(), CertStore::new()).unwrap();
	engine.start().await.unwrap();
	assert!(engine.listener_addrs().is_empty());

	// Add a port via update_config
	let new_config = make_config(HashMap::from([(0, make_port_config(echo.addr()))]));
	engine.update_config(new_config).await.unwrap();

	let listen_addr = engine.listener_addr(0).unwrap();

	let mut client = TcpStream::connect(listen_addr).await.unwrap();
	client.write_all(b"added port").await.unwrap();
	let mut buf = Vec::new();
	client.read_to_end(&mut buf).await.unwrap();
	assert_eq!(buf, b"added port");

	engine.shutdown();
	engine.join().await;
}

/// `update_config` that removes a port stops its listener.
#[tokio::test]
async fn update_config_removes_port() {
	let echo = EchoServer::start().await;

	let config = make_config(HashMap::from([(0, make_port_config(echo.addr()))]));
	let engine = Engine::new(config, make_registry(), CertStore::new()).unwrap();
	engine.start().await.unwrap();

	let listen_addr = engine.listener_addr(0).unwrap();

	// Verify connectivity
	{
		let mut client = TcpStream::connect(listen_addr).await.unwrap();
		client.write_all(b"before remove").await.unwrap();
		let mut buf = Vec::new();
		client.read_to_end(&mut buf).await.unwrap();
		assert_eq!(buf, b"before remove");
	}

	// Remove the port via update_config
	engine.update_config(ConfigTable::default()).await.unwrap();
	assert!(engine.listener_addr(0).is_none());

	tokio::time::sleep(Duration::from_millis(50)).await;

	let result =
		tokio::time::timeout(Duration::from_millis(200), TcpStream::connect(listen_addr)).await;
	assert!(result.is_err() || result.unwrap().is_err(), "connection should fail after port removed");

	engine.join().await;
}

/// `update_config` on a kept port hot-reloads the flow rule for the next connection.
#[tokio::test]
async fn update_config_hot_reload_kept_port() {
	let echo_a = EchoServer::start().await;
	let echo_b = EchoServer::start().await;

	let config_a = make_config(HashMap::from([(0, make_port_config(echo_a.addr()))]));
	let engine = Engine::new(config_a, make_registry(), CertStore::new()).unwrap();
	engine.start().await.unwrap();

	let listen_addr = engine.listener_addr(0).unwrap();

	// First connection goes through echo_a
	{
		let mut client = TcpStream::connect(listen_addr).await.unwrap();
		client.write_all(b"echo_a").await.unwrap();
		let mut buf = Vec::new();
		client.read_to_end(&mut buf).await.unwrap();
		assert_eq!(buf, b"echo_a");
	}

	// Hot-reload: point the same port at echo_b
	let config_b = make_config(HashMap::from([(0, make_port_config(echo_b.addr()))]));
	engine.update_config(config_b).await.unwrap();

	// Listener should still be on the same address (no restart)
	assert_eq!(engine.listener_addr(0).unwrap(), listen_addr);

	// Next connection goes through echo_b
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
