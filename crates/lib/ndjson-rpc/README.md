# Line-delimited JSON RPC

A small RPC framing crate that speaks `{ id, verb, args }` ↔
`{ id, result | error | event | end }` newline-delimited JSON, with
two interchangeable transports: a Unix-domain-socket server / client
for local control planes, and an HTTP/1.1 server / client that streams
the same frames as `Transfer-Encoding: chunked` NDJSON.

## Features

Daemon ↔ CLI control planes (containerd, caddy, kubelet, …) are a
recurring pattern that everyone re-implements. tower / jsonrpsee solve
a much bigger problem and pull in a lot of weight; this crate stays in
the "framed bytes + dispatch trait" lane and is small enough to fork
when you need to.

## Server

Implement [`Handler`] against your application state, hand an `Arc<H>`
to either [`spawn_unix_server`] or [`spawn_http_server`] (or both):

```rust,no_run
use std::sync::Arc;
use async_trait::async_trait;
use ndjson_rpc::{DispatchOutcome, Handler, Request, WireError, WireErrorKind, spawn_unix_server};
use tokio_util::sync::CancellationToken;

struct App;

#[async_trait]
impl Handler for App {
    async fn dispatch(&self, req: Request) -> DispatchOutcome {
        match req.verb.as_str() {
            "ping" => DispatchOutcome::OneShot(Ok(serde_json::json!({ "pong": true }))),
            other => DispatchOutcome::OneShot(Err(WireError {
                kind: WireErrorKind::UnknownVerb,
                message: format!("unknown verb: {other}"),
            })),
        }
    }
}

# async fn run() -> std::io::Result<()> {
let cancel = CancellationToken::new();
let _task = spawn_unix_server(
    std::path::Path::new("/run/myapp.sock"),
    Arc::new(App),
    cancel,
).await?;
# Ok(())
# }
```

Streaming verbs return `DispatchOutcome::Stream(Box<dyn EventStream + Send>)`;
the server frames each `Some(value)` as an `Event` and writes a final
`End` when the stream returns `None`. The client cancels by closing
the socket — the `EventStream` is dropped, and any cleanup it owns
runs through `Drop`.

## Client

[`UnixMgmtClient`] (Unix socket) and [`HttpMgmtClient`] (HTTP/1.1 +
optional bearer token) both expose `call(verb, args)` for one-shot
verbs and `call_stream(verb, args, on_event)` / `stream(...)` for
streaming verbs. Each call opens a fresh connection — the protocol is
verb-at-a-time and not chatty enough to amortize a connection pool.

## License

Released under the MIT License © 2026 [Canmi](https://canmi.net)
