#![allow(clippy::unwrap_used)]

use std::collections::HashMap;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use vane_engine::{
    config::FlowNode,
    engine::{Engine, EngineConfig},
    flow::{
        FlowTable, PluginAction, PluginRegistry, ProtocolDetect,
        builtin::tcp_forward::TcpForward,
    },
};
use vane_test_utils::echo::EchoServer;
use vane_transport::tcp::ProxyConfig;

/// Build a flow: protocol.detect -> {branch} -> tcp.forward(echo)
fn detect_flow(echo_addr: std::net::SocketAddr, branches: &[&str]) -> (FlowNode, PluginRegistry) {
    let forward_params = serde_json::json!({
        "ip": echo_addr.ip().to_string(),
        "port": echo_addr.port(),
    });

    let branch_map: HashMap<String, FlowNode> = branches
        .iter()
        .map(|b| {
            (
                (*b).to_owned(),
                FlowNode {
                    plugin: "tcp.forward".to_owned(),
                    params: forward_params.clone(),
                    branches: HashMap::new(),
                    termination: None,
                },
            )
        })
        .collect();

    let node = FlowNode {
        plugin: "protocol.detect".to_owned(),
        params: serde_json::Value::Null,
        branches: branch_map,
        termination: None,
    };

    let registry = PluginRegistry::new()
        .register(
            "protocol.detect",
            PluginAction::Middleware(Box::new(ProtocolDetect::with_defaults())),
        )
        .register(
            "tcp.forward",
            PluginAction::Terminator(Box::new(TcpForward {
                proxy_config: ProxyConfig::default(),
            })),
        );

    (node, registry)
}

/// TLS-like bytes are detected and routed through the "tls" branch.
#[tokio::test]
async fn detect_tls_routes_to_tls_branch() {
    let echo = EchoServer::start().await;
    let (node, registry) = detect_flow(echo.addr(), &["tls", "http", "unknown"]);

    let flow_table = FlowTable::new().add(0, node);
    let mut engine = Engine::new(flow_table, registry, EngineConfig::default());
    engine.start().await.unwrap();

    let listen_addr = engine.listeners()[0].local_addr();

    let mut client = TcpStream::connect(listen_addr).await.unwrap();
    // TLS ClientHello: 0x16 (Handshake) + 0x03 0x01 (TLS 1.0) + payload
    let tls_payload = [0x16, 0x03, 0x01, 0x00, 0x05, b'h', b'e', b'l', b'l', b'o'];
    client.write_all(&tls_payload).await.unwrap();

    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();
    assert_eq!(response, tls_payload);

    engine.shutdown();
    engine.join().await;
}

/// HTTP request is detected and routed through the "http" branch.
#[tokio::test]
async fn detect_http_routes_to_http_branch() {
    let echo = EchoServer::start().await;
    let (node, registry) = detect_flow(echo.addr(), &["tls", "http", "unknown"]);

    let flow_table = FlowTable::new().add(0, node);
    let mut engine = Engine::new(flow_table, registry, EngineConfig::default());
    engine.start().await.unwrap();

    let listen_addr = engine.listeners()[0].local_addr();

    let mut client = TcpStream::connect(listen_addr).await.unwrap();
    let http_data = b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n";
    client.write_all(http_data).await.unwrap();

    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();
    assert_eq!(response, http_data);

    engine.shutdown();
    engine.join().await;
}

/// Unknown protocol bytes are routed through the "unknown" fallback branch.
#[tokio::test]
async fn detect_unknown_routes_to_fallback_branch() {
    let echo = EchoServer::start().await;
    let (node, registry) = detect_flow(echo.addr(), &["tls", "http", "unknown"]);

    let flow_table = FlowTable::new().add(0, node);
    let mut engine = Engine::new(flow_table, registry, EngineConfig::default());
    engine.start().await.unwrap();

    let listen_addr = engine.listeners()[0].local_addr();

    let mut client = TcpStream::connect(listen_addr).await.unwrap();
    let random_data = [0xDE, 0xAD, 0xBE, 0xEF, 0x01, 0x02];
    client.write_all(&random_data).await.unwrap();

    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();
    assert_eq!(response, random_data);

    engine.shutdown();
    engine.join().await;
}
