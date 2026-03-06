use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use super::context::ExecutionContext;
use super::error::FlowError;
use super::plugin::PluginAction;
use super::registry::PluginRegistry;
use super::step::FlowStep;

/// Execute a flow step tree with a timeout wrapping the entire recursion.
pub async fn execute(
    step: &FlowStep,
    context: &mut dyn ExecutionContext,
    registry: &PluginRegistry,
    timeout: Duration,
) -> Result<(), FlowError> {
    tokio::time::timeout(timeout, execute_inner(step, context, registry))
        .await
        .map_err(|_| FlowError::ExecutionTimeout { timeout })?
}

fn execute_inner<'a>(
    step: &'a FlowStep,
    context: &'a mut dyn ExecutionContext,
    registry: &'a PluginRegistry,
) -> Pin<Box<dyn Future<Output = Result<(), FlowError>> + Send + 'a>> {
    Box::pin(async move {
        let plugin = registry
            .get(&step.plugin)
            .ok_or_else(|| FlowError::PluginNotFound {
                name: step.plugin.clone(),
            })?;

        match plugin {
            PluginAction::Middleware(mw) => {
                let action = mw
                    .execute(&step.config.params, &*context)
                    .map_err(|source| FlowError::PluginFailed {
                        name: step.plugin.clone(),
                        source,
                    })?;

                for (key, value) in action.updates {
                    context.kv_mut().set(key, value);
                }

                let next = step
                    .config
                    .branches
                    .get(&action.branch)
                    .ok_or_else(|| FlowError::BranchNotFound {
                        step: step.plugin.clone(),
                        branch: action.branch.clone(),
                    })?;

                execute_inner(next, context, registry).await
            }
            PluginAction::Terminator(term) => {
                let stream = context.take_stream().ok_or_else(|| {
                    FlowError::StreamAlreadyConsumed {
                        step: step.plugin.clone(),
                    }
                })?;

                term.execute(
                    &step.config.params,
                    context.kv(),
                    stream,
                    context.peer_addr(),
                    context.server_addr(),
                )
                .await
                .map_err(|source| FlowError::PluginFailed {
                    name: step.plugin.clone(),
                    source,
                })
            }
        }
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::flow::context::ExecutionContext;
    use crate::flow::plugin::{BranchAction, Middleware, PluginAction, Terminator};
    use crate::flow::step::StepConfig;
    use std::collections::HashMap;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use tokio::net::TcpStream;
    use vane_primitives::kv::KvStore;

    // -- helpers --

    fn test_addrs() -> (SocketAddr, SocketAddr) {
        let peer = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)), 12345);
        let server = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080);
        (peer, server)
    }

    struct MockContext {
        peer: SocketAddr,
        server: SocketAddr,
        kv: KvStore,
        stream: Option<TcpStream>,
    }

    impl ExecutionContext for MockContext {
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
        fn take_stream(&mut self) -> Option<TcpStream> {
            self.stream.take()
        }
    }

    async fn make_mock_context() -> MockContext {
        let (peer, server) = test_addrs();
        let kv = KvStore::new(&peer, &server, "tcp");

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (stream, _) = tokio::join!(TcpStream::connect(addr), listener.accept());

        MockContext {
            peer,
            server,
            kv,
            stream: Some(stream.unwrap()),
        }
    }

    // -- mock plugins --

    struct MockTerminator {
        called: Arc<AtomicBool>,
    }

    impl Terminator for MockTerminator {
        fn execute(
            &self,
            _params: &serde_json::Value,
            _kv: &KvStore,
            _stream: TcpStream,
            _peer_addr: SocketAddr,
            _server_addr: SocketAddr,
        ) -> Pin<Box<dyn Future<Output = Result<(), anyhow::Error>> + Send + '_>> {
            self.called.store(true, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        }
    }

    struct FixedBranchMiddleware {
        branch: String,
    }

    impl Middleware for FixedBranchMiddleware {
        fn execute(
            &self,
            _params: &serde_json::Value,
            _ctx: &dyn ExecutionContext,
        ) -> Result<BranchAction, anyhow::Error> {
            Ok(BranchAction {
                branch: self.branch.clone(),
                updates: vec![("visited".to_owned(), "true".to_owned())],
            })
        }
    }

    struct SlowTerminator;

    impl Terminator for SlowTerminator {
        fn execute(
            &self,
            _params: &serde_json::Value,
            _kv: &KvStore,
            _stream: TcpStream,
            _peer_addr: SocketAddr,
            _server_addr: SocketAddr,
        ) -> Pin<Box<dyn Future<Output = Result<(), anyhow::Error>> + Send + '_>> {
            Box::pin(async {
                tokio::time::sleep(Duration::from_secs(60)).await;
                Ok(())
            })
        }
    }

    // -- tests --

    #[tokio::test]
    async fn single_terminator_step() {
        let called = Arc::new(AtomicBool::new(false));
        let registry = PluginRegistry::new().register(
            "mock.term",
            PluginAction::Terminator(Box::new(MockTerminator {
                called: called.clone(),
            })),
        );

        let step = FlowStep {
            plugin: "mock.term".to_owned(),
            config: StepConfig::default(),
        };

        let mut ctx = make_mock_context().await;
        let result = execute(&step, &mut ctx, &registry, Duration::from_secs(5)).await;
        assert!(result.is_ok());
        assert!(called.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn middleware_then_terminator() {
        let called = Arc::new(AtomicBool::new(false));
        let registry = PluginRegistry::new()
            .register(
                "mock.branch",
                PluginAction::Middleware(Box::new(FixedBranchMiddleware {
                    branch: "next".to_owned(),
                })),
            )
            .register(
                "mock.term",
                PluginAction::Terminator(Box::new(MockTerminator {
                    called: called.clone(),
                })),
            );

        let step = FlowStep {
            plugin: "mock.branch".to_owned(),
            config: StepConfig {
                params: serde_json::Value::Null,
                branches: HashMap::from([(
                    "next".to_owned(),
                    FlowStep {
                        plugin: "mock.term".to_owned(),
                        config: StepConfig::default(),
                    },
                )]),
            },
        };

        let mut ctx = make_mock_context().await;
        let result = execute(&step, &mut ctx, &registry, Duration::from_secs(5)).await;
        assert!(result.is_ok());
        assert!(called.load(Ordering::SeqCst));
        // Middleware applied KV updates
        assert_eq!(ctx.kv().get("visited"), Some("true"));
    }

    #[tokio::test]
    async fn missing_plugin_error() {
        let registry = PluginRegistry::new();
        let step = FlowStep {
            plugin: "nonexistent".to_owned(),
            config: StepConfig::default(),
        };

        let mut ctx = make_mock_context().await;
        let result = execute(&step, &mut ctx, &registry, Duration::from_secs(5)).await;
        assert!(matches!(result, Err(FlowError::PluginNotFound { .. })));
    }

    #[tokio::test]
    async fn missing_branch_error() {
        let registry = PluginRegistry::new().register(
            "mock.branch",
            PluginAction::Middleware(Box::new(FixedBranchMiddleware {
                branch: "missing".to_owned(),
            })),
        );

        let step = FlowStep {
            plugin: "mock.branch".to_owned(),
            config: StepConfig {
                params: serde_json::Value::Null,
                branches: HashMap::new(), // no branches defined
            },
        };

        let mut ctx = make_mock_context().await;
        let result = execute(&step, &mut ctx, &registry, Duration::from_secs(5)).await;
        assert!(matches!(result, Err(FlowError::BranchNotFound { .. })));
    }

    #[tokio::test]
    async fn timeout_triggers() {
        let registry = PluginRegistry::new().register(
            "slow",
            PluginAction::Terminator(Box::new(SlowTerminator)),
        );

        let step = FlowStep {
            plugin: "slow".to_owned(),
            config: StepConfig::default(),
        };

        let mut ctx = make_mock_context().await;
        // Use a short timeout; SlowTerminator sleeps 60s so this will fire first
        let result = execute(
            &step,
            &mut ctx,
            &registry,
            Duration::from_millis(50),
        )
        .await;
        assert!(matches!(result, Err(FlowError::ExecutionTimeout { .. })));
    }

    #[tokio::test]
    async fn stream_already_consumed() {
        let registry = PluginRegistry::new().register(
            "mock.term",
            PluginAction::Terminator(Box::new(MockTerminator {
                called: Arc::new(AtomicBool::new(false)),
            })),
        );

        let step = FlowStep {
            plugin: "mock.term".to_owned(),
            config: StepConfig::default(),
        };

        let mut ctx = make_mock_context().await;
        // Pre-consume the stream
        let _taken = ctx.take_stream();

        let result = execute(&step, &mut ctx, &registry, Duration::from_secs(5)).await;
        assert!(matches!(
            result,
            Err(FlowError::StreamAlreadyConsumed { .. })
        ));
    }
}
