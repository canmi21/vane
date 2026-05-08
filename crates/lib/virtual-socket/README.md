# virtual-socket

Virtual UDP sockets that share a single physical
[`tokio::net::UdpSocket`].

A top-level "router" task owns the physical socket and reads from it.
For each inbound datagram, the router applies whatever demultiplex
rule the calling system needs (peer address, QUIC Connection ID, DNS
query ID, listener kind, ...) and pushes the datagram onto the
matching `VirtualUdpSocket`'s bounded inbound queue. Consumers drain
that queue.

Outbound is mux: every virtual socket forwards `try_send_to` /
`poll_send_ready` to the shared physical socket.

The crate is transport-policy free — it does not parse datagrams,
does not own a routing table, and does not implement any application
protocol. Pair it with whatever demultiplex strategy the calling
system needs.

## Example

```rust
use std::net::SocketAddr;
use std::sync::Arc;
use bytes::Bytes;
use tokio::net::UdpSocket;
use virtual_socket::VirtualUdpSocket;

# async fn run() -> std::io::Result<()> {
let physical = Arc::new(UdpSocket::bind("0.0.0.0:0").await?);
let virt = VirtualUdpSocket::new(Arc::clone(&physical));

// Router (separate task) recv_from's `physical` and routes to `virt`:
//   virt.enqueue_inbound(peer, datagram);

// Consumer drains:
//   while let Some((peer, datagram)) = virt.try_dequeue() { ... }
# Ok(())
# }
```

## See also

- [`quinn-shared-socket`](https://crates.io/crates/quinn-shared-socket)
  — adapter that exposes a `VirtualUdpSocket` as a
  `quinn::AsyncUdpSocket`, so a `quinn::Endpoint` can run on top.

## License

MIT.
