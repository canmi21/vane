# Hyper CGI

Async helpers for the slice of an RFC 3875 CGI driver that doesn't
need `unsafe`: parse the child's stdout into an `http::Response`,
stream the body through `http_body::Body`, and recognise the env
keys an operator must not override.

This crate is a building block, not a complete CGI runner — the
fork-exec / `pre_exec` / privilege-drop / rlimits bits stay in the
host, where the `unsafe` boundary can be audited in context. What
lives here is everything that's mechanically reusable across CGI
drivers regardless of how the child was spawned.

## What's included

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
  permit / Arc / cancellation guard alive for the body's lifetime.
- [`is_reserved_env_key`] — predicate for the operator-config
  validator: returns true when a key collides with an
  RFC 3875 / common-extension variable or the `HTTP_*` request-
  header passthrough namespace.

## Example

```rust,no_run
use std::time::Duration;
use bytes::Bytes;
use hyper_cgi::{parse_response_headers, read_until_header_end, CgiResponseBody};
use tokio::time::Instant;

# async fn drive<R, G>(stdout: R, guard: G) -> Result<http::Response<CgiResponseBody<R, G>>, Box<dyn std::error::Error>>
# where
#     R: tokio::io::AsyncRead + Unpin + Send,
#     G: Send + 'static,
# {
let connect_deadline = Instant::now() + Duration::from_secs(5);
let total_deadline = connect_deadline + Duration::from_secs(60);

let (header_block, leftover, stdout) = read_until_header_end(stdout, connect_deadline).await?;
let resp = parse_response_headers(&header_block)?
    .body(CgiResponseBody::new(leftover, stdout, total_deadline, guard))?;
# Ok(resp)
# }
```

## What's not included

- The `unsafe` `pre_exec` closure that drops privileges + applies
  rlimits — host-specific and audit-sensitive.
- The fork-exec spawn loop and stderr drain — trivial wiring.
- The RFC 3875 environment builder — every host serializes a
  different request shape (vane's `Request` + `ConnContext`,
  hyper's `Request<Incoming>`, axum's extractors, …) so one
  generic helper would just constrain. Use [`is_reserved_env_key`]
  to validate operator-supplied env entries against the names this
  crate's neighbours are known to compute per request.

## License

Released under the MIT License © 2026 [Canmi](https://canmi.net)
