use crate::flow::context::ExecutionContext;
use crate::flow::plugin::{BranchAction, Middleware};

/// Middleware that reads branch name from params and sets a KV marker.
///
/// Params (all optional):
/// - `branch`: which branch to follow (default: `"default"`)
/// - `key`: KV key to set (default: `"echo.visited"`)
/// - `value`: KV value to set (default: `"true"`)
pub struct EchoBranch;

impl Middleware for EchoBranch {
    fn execute(
        &self,
        params: &serde_json::Value,
        _ctx: &dyn ExecutionContext,
    ) -> Result<BranchAction, anyhow::Error> {
        let branch = params
            .get("branch")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("default")
            .to_owned();

        let key = params
            .get("key")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("echo.visited")
            .to_owned();

        let value = params
            .get("value")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("true")
            .to_owned();

        Ok(BranchAction {
            branch,
            updates: vec![(key, value)],
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use vane_primitives::kv::KvStore;

    struct DummyContext {
        peer: SocketAddr,
        server: SocketAddr,
        kv: KvStore,
    }

    impl ExecutionContext for DummyContext {
        fn peer_addr(&self) -> SocketAddr {
            self.peer
        }
        fn server_addr(&self) -> SocketAddr {
            self.server
        }
        fn kv(&self) -> &KvStore {
            &self.kv
        }
        fn kv_mut(&mut self) -> &mut KvStore {
            &mut self.kv
        }
        fn take_stream(&mut self) -> Option<tokio::net::TcpStream> {
            None
        }
    }

    fn dummy_ctx() -> DummyContext {
        let peer = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)), 12345);
        let server = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080);
        let kv = KvStore::new(&peer, &server, "tcp");
        DummyContext { peer, server, kv }
    }

    #[test]
    fn default_params() {
        let ctx = dummy_ctx();

        let result = EchoBranch
            .execute(&serde_json::Value::Null, &ctx)
            .expect("should succeed");

        assert_eq!(result.branch, "default");
        assert_eq!(result.updates.len(), 1);
        assert_eq!(result.updates[0].0, "echo.visited");
        assert_eq!(result.updates[0].1, "true");
    }

    #[test]
    fn custom_params() {
        let ctx = dummy_ctx();

        let params = serde_json::json!({
            "branch": "custom",
            "key": "my.key",
            "value": "yes"
        });

        let result = EchoBranch
            .execute(&params, &ctx)
            .expect("should succeed");

        assert_eq!(result.branch, "custom");
        assert_eq!(result.updates[0].0, "my.key");
        assert_eq!(result.updates[0].1, "yes");
    }
}
