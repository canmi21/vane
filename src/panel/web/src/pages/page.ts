export const loaders = {
  connections: { procedure: "listConnections" }
}

export const mock = {
  connections: {
    total: 2,
    connections: [
      {
        id: "conn_demo_alpha",
        peer_addr: "127.0.0.1:54820",
        server_addr: "127.0.0.1:443",
        listen_port: 443,
        layer: "l7",
        phase: "forwarding",
        protocol: "http",
        tls_sni: "demo.vane.local",
        tls_version: "TLSv1.3",
        forward_target: "127.0.0.1:3000",
        started_at_unix_ms: "1735689600000"
      },
      {
        id: "conn_demo_beta",
        peer_addr: "127.0.0.1:54824",
        server_addr: "127.0.0.1:8443",
        listen_port: 8443,
        layer: "l4",
        phase: "accepted",
        protocol: null,
        tls_sni: null,
        tls_version: null,
        forward_target: "127.0.0.1:8080",
        started_at_unix_ms: "1735689700000"
      }
    ]
  }
}
