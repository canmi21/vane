# H3 Body

An `http_body::Body` adapter over the [h3] crate's split
`recv_data` / `recv_trailers` stream surface, for both the server-side
`h3::server::RequestStream` and the client-side
`h3::client::RequestStream`.

`h3` exposes its body shape as two separate calls — one returning
`impl Buf` chunks and a once-only `recv_trailers()` after the data
half closes. `H3Body` runs a small pump task that walks both calls in
order and feeds a bounded channel; the resulting `http_body::Body`
slots into hyper / tower / any HTTP stack that expects the standard
trait.

[h3]: https://crates.io/crates/h3

## Example

```rust,no_run
use h3_body::{H3Body, ServerStreamSource};
use http_body::Body as _;

# async fn handle<S>(req_stream: h3::server::RequestStream<S, bytes::Bytes>) -> std::io::Result<()>
# where
#     S: h3::quic::RecvStream + Send + 'static,
# {
let body = H3Body::new(ServerStreamSource::new(req_stream));
// `body: impl http_body::Body<Data = Bytes, Error = io::Error>`
// — feed it to whatever your HTTP stack expects.
# Ok(())
# }
```

The `Error` type is `std::io::Error`; h3's transport / decoding
errors are wrapped via `io::Error::other(...)` so callers don't need
to depend on h3's error types to handle them.

## License

Released under the MIT License © 2026 [Canmi](https://canmi.net)
