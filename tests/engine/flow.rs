#![allow(clippy::unwrap_used)]

use std::collections::HashMap;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use vane_engine::{
    engine::{Engine, EngineConfig},
    flow::{
        FlowStep, FlowTable, PluginAction, PluginRegistry, StepConfig,
        builtin::{echo_branch::EchoBranch, tcp_forward::TcpForward},
    },
};
use vane_test_utils::echo::EchoServer;
use vane_transport::tcp::ProxyConfig;

/// Multi-step flow: echo.branch middleware -> tcp.forward terminator.
#[tokio::test]
async fn test_multi_step_flow() {
    let echo = EchoServer::start().await;
    let echo_addr = echo.addr();

    let step = FlowStep {
        plugin: "echo.branch".to_owned(),
        config: StepConfig {
            params: serde_json::json!({"branch": "default"}),
            branches: HashMap::from([(
                "default".to_owned(),
                FlowStep {
                    plugin: "tcp.forward".to_owned(),
                    config: StepConfig {
                        params: serde_json::json!({
                            "ip": echo_addr.ip().to_string(),
                            "port": echo_addr.port(),
                        }),
                        ..Default::default()
                    },
                },
            )]),
        },
    };

    let flow_table = FlowTable::new().add(0, step);
    let registry = PluginRegistry::new()
        .register(
            "echo.branch",
            PluginAction::Middleware(Box::new(EchoBranch)),
        )
        .register(
            "tcp.forward",
            PluginAction::Terminator(Box::new(TcpForward {
                proxy_config: ProxyConfig::default(),
            })),
        );

    let mut engine = Engine::new(flow_table, registry, EngineConfig::default());
    engine.start().await.unwrap();

    let listen_addr = engine.listeners()[0].local_addr();

    let mut client = TcpStream::connect(listen_addr).await.unwrap();
    client.write_all(b"multi step").await.unwrap();

    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();

    assert_eq!(response, b"multi step");

    engine.shutdown();
    engine.join().await;
}

/// Missing branch: middleware returns a branch name not present in branches map.
/// The engine should log the error, not panic.
#[tokio::test]
async fn test_missing_branch_does_not_panic() {
    let step = FlowStep {
        plugin: "echo.branch".to_owned(),
        config: StepConfig {
            params: serde_json::json!({"branch": "nonexistent"}),
            branches: HashMap::new(), // no branches defined
        },
    };

    let flow_table = FlowTable::new().add(0, step);
    let registry = PluginRegistry::new().register(
        "echo.branch",
        PluginAction::Middleware(Box::new(EchoBranch)),
    );

    let mut engine = Engine::new(flow_table, registry, EngineConfig::default());
    engine.start().await.unwrap();

    let listen_addr = engine.listeners()[0].local_addr();

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
            _stream: tokio::net::TcpStream,
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

    let step = FlowStep {
        plugin: "never".to_owned(),
        config: StepConfig::default(),
    };

    let flow_table = FlowTable::new().add(0, step);
    let registry = PluginRegistry::new().register(
        "never",
        PluginAction::Terminator(Box::new(NeverTerminator)),
    );

    let config = EngineConfig {
        flow_timeout: Duration::from_millis(100),
        ..Default::default()
    };

    let mut engine = Engine::new(flow_table, registry, config);
    engine.start().await.unwrap();

    let listen_addr = engine.listeners()[0].local_addr();

    let mut client = TcpStream::connect(listen_addr).await.unwrap();
    client.write_all(b"timeout test").await.unwrap();

    let mut response = Vec::new();
    // Server should drop the connection after timeout
    let _ = client.read_to_end(&mut response).await;

    engine.shutdown();
    engine.join().await;
}
