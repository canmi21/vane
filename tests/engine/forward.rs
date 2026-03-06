use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use vane_engine::{
    engine::Engine,
    rule::{ForwardRule, RouteTable},
};
use vane_test_utils::echo::EchoServer;
use vane_transport::tcp::ProxyConfig;

#[tokio::test]
async fn test_echo_forward() {
    let echo = EchoServer::start().await;

    let route_table = RouteTable::new().add(
        0,
        ForwardRule {
            upstream: echo.addr(),
            proxy_config: ProxyConfig::default(),
        },
    );

    let mut engine = Engine::new(route_table);
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
