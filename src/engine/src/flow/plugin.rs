use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;

use tokio::net::TcpStream;
use vane_primitives::kv::KvStore;

use super::context::ExecutionContext;

/// Result of a middleware step: which branch to follow and KV updates to apply.
pub struct BranchAction {
	pub branch: String,
	pub updates: Vec<(String, String)>,
}

/// A synchronous plugin that inspects connection context and decides which branch to take.
pub trait Middleware: Send + Sync {
	fn execute(
		&self,
		params: &serde_json::Value,
		ctx: &dyn ExecutionContext,
	) -> Result<BranchAction, anyhow::Error>;
}

/// An async plugin that consumes the TCP stream and terminates the flow.
pub trait Terminator: Send + Sync {
	fn execute(
		&self,
		params: &serde_json::Value,
		kv: &KvStore,
		stream: TcpStream,
		peer_addr: SocketAddr,
		server_addr: SocketAddr,
	) -> Pin<Box<dyn Future<Output = Result<(), anyhow::Error>> + Send + '_>>;
}

/// Wraps either a middleware or terminator plugin for dynamic dispatch.
pub enum PluginAction {
	Middleware(Box<dyn Middleware>),
	Terminator(Box<dyn Terminator>),
}
