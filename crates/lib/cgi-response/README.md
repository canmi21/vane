# CGI Response

Parse a CGI child's stdout into an `http::Response`, then stream
the body through `http_body::Body`. The other half of a CGI
gateway — building the RFC 3875 environment for the child — lives
in the [`cgi-request`](https://crates.io/crates/cgi-request) crate; pair them when you need both
directions.

This crate is a building block, not a complete CGI runner — the
fork-exec / `pre_exec` / privilege-drop / rlimits bits stay in the
host, where the `unsafe` boundary can be audited in context. What
lives here is everything around the response that's mechanically
reusable regardless of how the child was spawned.

## Features

- [`read_until_header_end`] — read an `AsyncRead` until the
  RFC 3875 header / body separator (`\r\n\r\n`), with a deadline.
  Returns the header block, the leftover post-separator bytes, and
  the still-open reader for body streaming.
- [`parse_response_headers`] — turn a header block into an
  `http::response::Builder`. Handles the `Status:` CGI-specific
  header, the `Location:`-without-`Status:` ⇒ 302 fallback, and
  the no-`Status:` ⇒ 200 default.
- [`CgiResponseBody`] — `http_body::Body` impl that yields the
  leftover bytes first, then poll-reads from the open reader to
  EOF. Generic over a drop-guard parameter so the host can hold a
  permit / `Arc` / cancellation guard alive for the body's
  lifetime.

## Example

```rust,no_run
use std::time::Duration;
use cgi_response::{parse_response_headers, read_until_header_end, CgiResponseBody};
use tokio::time::Instant;

# async fn drive<R, G>(stdout: R, guard: G) -> Result<http::Response<CgiResponseBody<R, G>>, Box<dyn std::error::Error>>
# where
#     R: tokio::io::AsyncRead + Unpin + Send,
#     G: Send + Unpin + 'static,
# {
let connect_deadline = Instant::now() + Duration::from_secs(5);
let total_deadline = connect_deadline + Duration::from_secs(60);

let (header_block, leftover, stdout) = read_until_header_end(stdout, connect_deadline).await?;
let resp = parse_response_headers(&header_block)?
    .body(CgiResponseBody::new(leftover, stdout, total_deadline, guard))?;
# Ok(resp)
# }
```

## License

Released under the MIT License © 2026 [Canmi](https://canmi.net)
