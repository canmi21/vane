use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;

use tokio::net::TcpStream;
use vane_primitives::kv::KvStore;
use vane_primitives::model::ResolvedTarget;
use vane_transport::tcp::{ProxyConfig, proxy_tcp};

use crate::flow::plugin::Terminator;

/// Terminator that forwards a TCP connection to a resolved target.
///
/// Target resolution order:
/// 1. `params.ip` + `params.port` (explicit in step config)
/// 2. KV keys `target.ip` + `target.port` (set by upstream middleware)
pub struct TcpForward {
    pub proxy_config: ProxyConfig,
}

impl TcpForward {
    fn resolve_target(
        params: &serde_json::Value,
        kv: &KvStore,
    ) -> Result<ResolvedTarget, anyhow::Error> {
        // Try params first
        if let (Some(ip), Some(port)) = (
            params.get("ip").and_then(serde_json::Value::as_str),
            params.get("port").and_then(serde_json::Value::as_u64),
        ) {
            let ip: IpAddr = ip.parse().map_err(|e| anyhow::anyhow!("invalid ip in params: {e}"))?;
            let port = u16::try_from(port).map_err(|e| anyhow::anyhow!("invalid port in params: {e}"))?;
            return Ok(ResolvedTarget {
                addr: SocketAddr::new(ip, port),
            });
        }

        // Fall back to KV store
        if let (Some(ip_str), Some(port_str)) = (kv.get("target.ip"), kv.get("target.port")) {
            let ip: IpAddr = ip_str
                .parse()
                .map_err(|e| anyhow::anyhow!("invalid target.ip in kv: {e}"))?;
            let port: u16 = port_str
                .parse()
                .map_err(|e| anyhow::anyhow!("invalid target.port in kv: {e}"))?;
            return Ok(ResolvedTarget {
                addr: SocketAddr::new(ip, port),
            });
        }

        Err(anyhow::anyhow!(
            "no target found: set params.ip+params.port or kv target.ip+target.port"
        ))
    }
}

impl Terminator for TcpForward {
    fn execute(
        &self,
        params: &serde_json::Value,
        kv: &KvStore,
        stream: TcpStream,
        _peer_addr: SocketAddr,
        _server_addr: SocketAddr,
    ) -> Pin<Box<dyn Future<Output = Result<(), anyhow::Error>> + Send + '_>> {
        let resolved = Self::resolve_target(params, kv);
        Box::pin(async move {
            let resolved = resolved?;
            proxy_tcp(stream, &resolved, &self.proxy_config).await?;
            Ok(())
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn test_addrs() -> (SocketAddr, SocketAddr) {
        let peer = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)), 12345);
        let server = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080);
        (peer, server)
    }

    #[test]
    fn resolve_from_params() {
        let (peer, server) = test_addrs();
        let kv = KvStore::new(&peer, &server, "tcp");
        let params = serde_json::json!({"ip": "10.0.0.1", "port": 443});

        let result = TcpForward::resolve_target(&params, &kv).unwrap();
        assert_eq!(result.addr, "10.0.0.1:443".parse::<SocketAddr>().unwrap());
    }

    #[test]
    fn resolve_from_kv() {
        let (peer, server) = test_addrs();
        let mut kv = KvStore::new(&peer, &server, "tcp");
        kv.set("target.ip".to_owned(), "172.16.0.1".to_owned());
        kv.set("target.port".to_owned(), "9090".to_owned());

        let result =
            TcpForward::resolve_target(&serde_json::Value::Null, &kv).unwrap();
        assert_eq!(
            result.addr,
            "172.16.0.1:9090".parse::<SocketAddr>().unwrap()
        );
    }

    #[test]
    fn resolve_missing_returns_error() {
        let (peer, server) = test_addrs();
        let kv = KvStore::new(&peer, &server, "tcp");

        let result = TcpForward::resolve_target(&serde_json::Value::Null, &kv);
        assert!(result.is_err());
    }

    #[test]
    fn params_takes_priority_over_kv() {
        let (peer, server) = test_addrs();
        let mut kv = KvStore::new(&peer, &server, "tcp");
        kv.set("target.ip".to_owned(), "172.16.0.1".to_owned());
        kv.set("target.port".to_owned(), "9090".to_owned());

        let params = serde_json::json!({"ip": "10.0.0.1", "port": 443});
        let result = TcpForward::resolve_target(&params, &kv).unwrap();
        assert_eq!(result.addr, "10.0.0.1:443".parse::<SocketAddr>().unwrap());
    }
}
