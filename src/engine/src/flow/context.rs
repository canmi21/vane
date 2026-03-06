use std::net::SocketAddr;

use bytes::Bytes;
use tokio::net::TcpStream;
use vane_primitives::kv::KvStore;

/// Provides access to per-connection state during flow execution.
pub trait ExecutionContext: Send {
    fn peer_addr(&self) -> SocketAddr;
    fn server_addr(&self) -> SocketAddr;
    fn kv(&self) -> &KvStore;
    fn kv_mut(&mut self) -> &mut KvStore;
    fn take_stream(&mut self) -> Option<TcpStream>;

    /// Returns peeked bytes from the connection, if available.
    fn peek_data(&self) -> Option<&[u8]> {
        None
    }
}

/// Real connection context carrying a `TcpStream` and metadata.
pub struct TransportContext {
    peer_addr: SocketAddr,
    server_addr: SocketAddr,
    kv: KvStore,
    stream: Option<TcpStream>,
    peek_data: Option<Bytes>,
}

impl TransportContext {
    pub const fn new(
        peer_addr: SocketAddr,
        server_addr: SocketAddr,
        kv: KvStore,
        stream: TcpStream,
    ) -> Self {
        Self {
            peer_addr,
            server_addr,
            kv,
            stream: Some(stream),
            peek_data: None,
        }
    }

    pub fn set_peek_data(&mut self, data: Bytes) {
        self.peek_data = Some(data);
    }
}

impl ExecutionContext for TransportContext {
    fn peer_addr(&self) -> SocketAddr {
        self.peer_addr
    }

    fn server_addr(&self) -> SocketAddr {
        self.server_addr
    }

    fn kv(&self) -> &KvStore {
        &self.kv
    }

    fn kv_mut(&mut self) -> &mut KvStore {
        &mut self.kv
    }

    fn take_stream(&mut self) -> Option<TcpStream> {
        self.stream.take()
    }

    fn peek_data(&self) -> Option<&[u8]> {
        self.peek_data.as_deref()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn test_addrs() -> (SocketAddr, SocketAddr) {
        let peer = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)), 12345);
        let server = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080);
        (peer, server)
    }

    #[tokio::test]
    async fn context_getters() {
        let (peer, server) = test_addrs();
        let kv = KvStore::new(&peer, &server, "tcp");

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let connect = tokio::net::TcpStream::connect(addr);
        let accept = listener.accept();
        let (stream, _) = tokio::join!(connect, accept);
        let stream = stream.unwrap();

        let ctx = TransportContext::new(peer, server, kv, stream);
        assert_eq!(ctx.peer_addr(), peer);
        assert_eq!(ctx.server_addr(), server);
        assert_eq!(ctx.kv().conn_proto(), "tcp");
    }

    #[tokio::test]
    async fn take_stream_returns_some_then_none() {
        let (peer, server) = test_addrs();
        let kv = KvStore::new(&peer, &server, "tcp");

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let connect = tokio::net::TcpStream::connect(addr);
        let accept = listener.accept();
        let (stream, _) = tokio::join!(connect, accept);
        let stream = stream.unwrap();

        let mut ctx = TransportContext::new(peer, server, kv, stream);
        assert!(ctx.take_stream().is_some());
        assert!(ctx.take_stream().is_none());
    }
}
