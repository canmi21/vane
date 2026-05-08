# peeked-stream

An `AsyncRead` + `AsyncWrite` adapter that prepends a previously-peeked
byte buffer to the read side of a stream, while passing writes through
unchanged.

Useful for protocol-detecting servers: peek the first bytes of a fresh
connection, decide which decoder to engage (TLS / HTTP/1 / HTTP/2
preface / opaque L4), then hand the stream to that decoder. Whichever
consumer wakes up next observes the peeked bytes from offset zero — as
though no read had happened.

## Example

```rust
use bytes::Bytes;
use peeked_stream::PeekedStream;
use tokio::io::AsyncReadExt;

# async fn run<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(stream: S) -> std::io::Result<()> {
let peeked = Bytes::from_static(b"GET / HTTP/1.1\r\n");
let mut s = PeekedStream::new(peeked, stream);

let mut buf = [0u8; 16];
s.read_exact(&mut buf).await?; // sees "GET / HTTP/1.1\r\n" first
# Ok(())
# }
```

`PeekedStream::into_inner()` hands back `(remaining_buffer, inner)`
when you need the concrete inner type back (e.g. a `TcpStream` for
`set_nodelay` / `peer_addr`).

## License

MIT.
