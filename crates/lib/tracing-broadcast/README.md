# Tracing Broadcast

A `tracing_subscriber::Layer` that fans every emitted event into a
`tokio::sync::broadcast` channel as a serializable [`TracingFrame`]
(timestamp / level / target / message / structured fields).

Useful for any service that wants to expose a "tail my logs" stream
over a management API, RPC, or websocket — `BroadcastTracingLayer`
plugs alongside the normal `tracing_subscriber::fmt::Layer`, so user-
visible logging on stderr stays unchanged. Each subscriber gets its
own `broadcast::Receiver` with independent backlog tracking; slow
subscribers see `RecvError::Lagged(n)` and resume from the next
available frame.

## Example

```rust
use tracing_broadcast::BroadcastTracingLayer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

# async fn run() {
let layer = BroadcastTracingLayer::new();
let mut rx = layer.subscribe();

let _guard = tracing_subscriber::registry().with(layer.clone()).set_default();
tracing::info!(addr = "127.0.0.1", port = 8080_u64, "listener bound");

let frame = rx.recv().await.unwrap();
assert_eq!(frame.level, "INFO");
assert_eq!(frame.message, "listener bound");
assert_eq!(frame.fields["port"], 8080);
# }
```

`TracingFrame` derives `serde::Serialize` / `Deserialize`, so the
operator-facing transport (NDJSON, websocket text frames, etc.) is
just `serde_json::to_string(&frame)`.

## License

Released under the MIT License © 2026 [Canmi](https://canmi.net)
