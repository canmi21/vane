use std::net::SocketAddr;

use tokio::net::TcpListener;

/// Allocates a free TCP port on localhost and returns its address.
///
/// There is a brief TOCTOU window between allocation and use.
/// Prefer using port 0 directly with `EchoServer::start()` or
/// `MockTcpServer::start()` when possible.
pub async fn free_port() -> SocketAddr {
	let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
	listener.local_addr().unwrap()
}
