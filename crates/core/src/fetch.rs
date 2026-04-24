use std::pin::Pin;
use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::oneshot;

use crate::body::{Request, Response};
use crate::conn_context::ConnContext;
use crate::error::Error;
use crate::flow_ctx::FlowCtx;
use crate::l4::L4Conn;
use crate::middleware::CloseReason;

#[trait_variant::make(L7Fetch: Send)]
pub trait L7FetchLocal {
	async fn fetch(
		&self,
		req: Request,
		conn: &Arc<ConnContext>,
		ctx: &mut FlowCtx<'_>,
	) -> Result<L7FetchOutput, Error>;
}

#[trait_variant::make(L4Fetch: Send)]
pub trait L4FetchLocal {
	async fn fetch(
		&self,
		l4: L4Conn,
		conn: &Arc<ConnContext>,
		ctx: &mut FlowCtx<'_>,
	) -> Result<Tunnel, Error>;
}

pub enum L7FetchOutput {
	Response(Response),
	Tunnel(Tunnel),
}

pub struct Tunnel {
	pub client: Pin<Box<dyn AsyncReadWrite + Send>>,
	pub upstream: Pin<Box<dyn AsyncReadWrite + Send>>,
	pub close_reason_tx: Option<oneshot::Sender<CloseReason>>,
}

pub trait AsyncReadWrite: AsyncRead + AsyncWrite {}
impl<T: AsyncRead + AsyncWrite + ?Sized> AsyncReadWrite for T {}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub enum FetchKind {
	HttpProxy,
	HttpSynthesize,
	WebSocketUpgrade,
	L4Forward,
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub enum FetchPhase {
	L4,
	L7,
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub struct FetchOutputModes {
	pub response: bool,
	pub tunnel: bool,
}

#[derive(Clone, Debug)]
pub struct SymbolicFetchRef {
	pub kind: FetchKind,
	pub args: serde_json::Value,
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub enum Terminator {
	WriteHttpResponse,
	ByteTunnel,
}
