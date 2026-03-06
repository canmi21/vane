#![allow(clippy::unwrap_used)]

use std::collections::HashMap;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use vane_engine::{
    config::FlowNode,
    engine::{Engine, EngineConfig},
    flow::{
        FlowTable, PluginAction, PluginRegistry,
        builtin::tcp_forward::TcpForward,
    },
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

    let flow_table = FlowTable::new().add(0, node);
    let registry = PluginRegistry::new().register(
        "tcp.forward",
        PluginAction::Terminator(Box::new(TcpForward {
            proxy_config: ProxyConfig::default(),
        })),
    );

    let mut engine = Engine::new(flow_table, registry, EngineConfig::default());
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
