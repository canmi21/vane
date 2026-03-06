use bytes::Bytes;
use tokio::net::TcpStream;

const MAX_PEEK_LIMIT: usize = 8192;

/// Peek at initial bytes of a TCP stream without consuming them.
///
/// Returns up to `limit` bytes (capped at `MAX_PEEK_LIMIT`).
/// Subsequent reads on the stream will still see the peeked data.
pub async fn peek_tcp(stream: &TcpStream, limit: usize) -> std::io::Result<Bytes> {
    let limit = limit.min(MAX_PEEK_LIMIT);
    let mut buf = vec![0u8; limit];
    let n = stream.peek(&mut buf).await?;
    buf.truncate(n);
    Ok(Bytes::from(buf))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn peek_returns_initial_bytes() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let send_task = tokio::spawn(async move {
            let mut conn = TcpStream::connect(addr).await.unwrap();
            conn.write_all(b"hello peek").await.unwrap();
            conn
        });

        let (server, _) = listener.accept().await.unwrap();
        let _client = send_task.await.unwrap();

        let peeked = peek_tcp(&server, 64).await.unwrap();
        assert_eq!(&peeked[..], b"hello peek");
    }

    #[tokio::test]
    async fn data_still_readable_after_peek() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let send_task = tokio::spawn(async move {
            let mut conn = TcpStream::connect(addr).await.unwrap();
            conn.write_all(b"readable").await.unwrap();
            conn.shutdown().await.unwrap();
            conn
        });

        let (mut server, _) = listener.accept().await.unwrap();
        let _client = send_task.await.unwrap();

        // Peek first
        let peeked = peek_tcp(&server, 64).await.unwrap();
        assert_eq!(&peeked[..], b"readable");

        // Read should still return the same data
        let mut buf = Vec::new();
        server.read_to_end(&mut buf).await.unwrap();
        assert_eq!(buf, b"readable");
    }

    #[tokio::test]
    async fn limit_caps_returned_bytes() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let send_task = tokio::spawn(async move {
            let mut conn = TcpStream::connect(addr).await.unwrap();
            conn.write_all(b"longpayload").await.unwrap();
            conn
        });

        let (server, _) = listener.accept().await.unwrap();
        let _client = send_task.await.unwrap();

        let peeked = peek_tcp(&server, 4).await.unwrap();
        assert_eq!(peeked.len(), 4);
        assert_eq!(&peeked[..], b"long");
    }
}
