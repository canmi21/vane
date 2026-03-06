use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use vane_engine::{
    engine::Engine,
    rule::{ForwardRule, RouteTable},
};
use vane_transport::tcp::ProxyConfig;

#[tokio::test]
async fn test_echo_forward() {
    // 1. Start a mock echo server on a random port.
    let echo_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let echo_addr = echo_listener.local_addr().unwrap();

    tokio::spawn(async move {
        let (mut stream, _) = echo_listener.accept().await.unwrap();
        let mut buf = vec![0u8; 1024];
        let n = stream.read(&mut buf).await.unwrap();
        stream.write_all(&buf[..n]).await.unwrap();
        stream.shutdown().await.unwrap();
    });

    // 2. Build a RouteTable with one rule pointing to the echo server.
    let route_table = RouteTable::new().add(
        0,
        ForwardRule {
            upstream: echo_addr,
            proxy_config: ProxyConfig::default(),
        },
    );

    // 3. Create and start the engine.
    let mut engine = Engine::new(route_table);
    engine.start().await.unwrap();

    // 4. Get the engine's actual listening address.
    let listen_addr = engine.listeners()[0].local_addr();

    // 5. Connect a client, send data, and read the response.
    let mut client = TcpStream::connect(listen_addr).await.unwrap();
    client.write_all(b"hello vane").await.unwrap();

    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();

    // 6. Assert the echoed data matches.
    assert_eq!(response, b"hello vane");

    // 7. Shut down the engine.
    engine.shutdown();
    engine.join().await;
}
