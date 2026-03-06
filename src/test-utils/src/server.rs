use std::future::Future;
use std::net::SocketAddr;

use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

/// A single-connection TCP server with a custom handler for testing.
pub struct MockTcpServer {
    addr: SocketAddr,
    handle: JoinHandle<()>,
}

impl MockTcpServer {
    pub async fn start<F, Fut>(handler: F) -> Self
    where
        F: FnOnce(TcpStream, SocketAddr) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (stream, peer) = listener.accept().await.unwrap();
            handler(stream, peer).await;
        });
        Self { addr, handle }
    }

    pub const fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub async fn join(self) {
        let _ = self.handle.await;
    }
}
