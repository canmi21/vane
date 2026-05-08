# Tokio Bind Retry

Retry `tokio::net::TcpListener` / `UdpSocket` bind against a transient
kernel-side bind failure with bounded exponential backoff, while
honouring a [`tokio_util::sync::CancellationToken`] so a daemon
shutdown does not hang in the retry loop.

Useful during graceful restart, port-handover dances, or any time a
previous process's socket may still be in `TIME_WAIT`. tokio and
socket2 do not ship anything for this — most servers grow a hand-rolled
`loop { sleep; bind }` that ignores cancellation and the operator has
to kill `-9` to escape.

## Example

```rust,no_run
use std::net::SocketAddr;
use tokio_bind_retry::{Policy, tcp};
use tokio_util::sync::CancellationToken;

# async fn run() {
let cancel = CancellationToken::new();
let policy = Policy::default(); // 10 attempts, 100ms → 5s backoff
let addr: SocketAddr = "0.0.0.0:8080".parse().unwrap();

let listener = tcp(addr, &cancel, &policy, 1024).await
    .expect("bind retries exhausted or cancelled");

// ... `listener.accept()` loop here ...
# }
```

The TCP path sets `SO_REUSEADDR` best-effort (silently ignored on
platforms where it is not permitted) and calls `listen(backlog)` once
the socket is bound. The UDP path is `UdpSocket::bind` directly. Both
return `None` on either retry exhaustion or cancellation — the caller
distinguishes by checking the token.

## Sleep or Cancel

The same primitive used internally is exposed as
[`sleep_or_cancel`] for non-bind retry loops sharing the same
cancellation token (e.g. an `accept` loop backing off on `EMFILE`):

```rust
use std::time::Duration;
use tokio_bind_retry::sleep_or_cancel;
use tokio_util::sync::CancellationToken;

# async fn run() {
let cancel = CancellationToken::new();
let cut_short = sleep_or_cancel(Duration::from_millis(100), &cancel).await;
if cut_short {
    return; // cancellation fired; bail out
}
# }
```

## License

Released under the MIT License © 2026 [Canmi](https://canmi.net)
