# Quinn Shared Socket

Run a `quinn::Endpoint` on a UDP socket that is shared with other
consumers, by adapting
[`virtual-socket`](https://crates.io/crates/virtual-socket)'s
`VirtualUdpSocket` to `quinn::AsyncUdpSocket`.

Use case: a single physical UDP listener carries multiple kinds of
traffic (QUIC + L4 forwarding + DNS + ...) and you don't want to give
quinn exclusive ownership of the socket. Pair this crate with
`virtual-socket` for the inbound demultiplex layer.

## Example

```rust
use std::sync::Arc;
use quinn_shared_socket::SharedSocket;
use virtual_socket::VirtualUdpSocket;

# async fn run(
#     physical: Arc<tokio::net::UdpSocket>,
#     server_config: quinn::ServerConfig,
# ) -> Result<(), Box<dyn std::error::Error>> {
let virt = VirtualUdpSocket::new(physical);
// ... your router task pushes inbound QUIC datagrams via virt.enqueue_inbound(peer, datagram) ...

let shared = SharedSocket::new(Arc::clone(&virt));
let endpoint = quinn::Endpoint::new_with_abstract_socket(
    quinn::EndpointConfig::default(),
    Some(server_config),
    shared,
    Arc::new(quinn::TokioRuntime),
)?;
# Ok(())
# }
```

## License

Released under the MIT License © 2026 [Canmi](https://canmi.net)
