use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use vane_engine::{
    engine::{Engine, EngineConfig},
    rule::{PortRule, RouteTable},
};
use vane_primitives::model::{Forward, Strategy, Target};
use vane_test_utils::echo::EchoServer;
use vane_transport::tcp::ProxyConfig;

#[tokio::test]
async fn test_echo_forward() {
    let echo = EchoServer::start().await;
    let echo_addr = echo.addr();

    let forward = Forward {
        strategy: Strategy::Random,
        targets: vec![Target::Ip {
            ip: echo_addr.ip(),
            port: echo_addr.port(),
        }],
        fallbacks: vec![],
    };

    let route_table = RouteTable::new().add(
        0,
        PortRule {
            forward,
            proxy_config: ProxyConfig::default(),
        },
    );

    let mut engine = Engine::new(route_table, EngineConfig::default());
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
