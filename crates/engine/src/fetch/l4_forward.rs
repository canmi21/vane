//! `L4ForwardFetch` — TCP→TCP byte forwarding.
//!
//! Connects to a configured upstream `host:port` per request and builds a
//! [`Tunnel`] for the executor's `Terminator::ByteTunnel` arm to drive
//! via `tokio::io::copy_bidirectional`. UDP session forwarding (5-tuple
//! demultiplex + idle-timeout reclaim) is out of scope this round and
//! lands with S2-11.
//!
//! See `spec/architecture/06-l4.md` § _`l4_forward`_. Feature: S1-18.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::net::TcpStream;
use vane_core::{
	AsyncReadWrite, ConnContext, Error, FetchKind, FlowCtx, L4Conn, L4Fetch, Tunnel, UpstreamReason,
};

use crate::factories::{FactoryError, FetchFactories};
use crate::flow_graph::FetchInst;

/// Connects per-request to a literal `host:port` upstream and hands the
/// executor a [`Tunnel`] that pairs the inbound client socket with the
/// freshly-dialed upstream. The executor's `Terminator::ByteTunnel`
/// terminator drives the bidirectional copy.
pub struct L4ForwardFetch {
	/// Resolved per-request via tokio's built-in `getaddrinfo`. DNS
	/// caching, multi-A round-robin, happy-eyeballs, and TCP keepalive
	/// configuration land later (S1-30 / S2). Stored as `String` rather
	/// than `SocketAddr` so symbolic hostnames work; resolution happens
	/// inside `tokio::net::TcpStream::connect`.
	upstream: String,
}

#[async_trait]
impl L4Fetch for L4ForwardFetch {
	async fn fetch(
		&self,
		l4: L4Conn,
		_conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<Tunnel, Error> {
		let client = match l4 {
			L4Conn::Tcp(s) => s,
			L4Conn::Udp(_) => {
				return Err(Error::internal(
					"UDP forward not supported in S1 — udp_dispatch + 5-tuple session land with S2-11",
				));
			}
		};

		let upstream = TcpStream::connect(&self.upstream)
			.await
			.map_err(|e| Error::upstream(UpstreamReason::Unreachable).with_source(e))?;

		// Disable Nagle on both sides — we're a transparent pipe and want
		// latency over throughput. Same default as nginx / haproxy L4
		// forward. Failure is silent (some platforms / namespaces deny
		// the syscall); it doesn't affect forwarding correctness.
		let _ = client.set_nodelay(true);
		let _ = upstream.set_nodelay(true);

		Ok(Tunnel {
			client: Box::new(client) as Box<dyn AsyncReadWrite + Send>,
			upstream: Box::new(upstream) as Box<dyn AsyncReadWrite + Send>,
			// L4 forward doesn't observe close reason; the executor's
			// `Terminator::ByteTunnel` arm sees `None` and skips the
			// oneshot send.
			close_reason_tx: None,
		})
	}
}

/// Args parser exposed as a registry-friendly factory. The expected
/// shape is:
///
/// ```json
/// { "upstream": "host:port" }
/// ```
///
/// Any other key is ignored (forward-compatible with future fields like
/// `idle_timeout`, `tcp_keepalive`, `dns_cache_ttl`).
///
/// # Errors
/// Returns [`FactoryError`] when `upstream` is missing, not a string, or
/// empty. Wider validation (literal `host:port` parse, port range) is
/// deferred to runtime — `tokio::net::TcpStream::connect` produces a
/// pointed error there.
pub fn factory(args: &serde_json::Value) -> Result<FetchInst, FactoryError> {
	let upstream = args
		.get("upstream")
		.and_then(serde_json::Value::as_str)
		.ok_or_else(|| FactoryError("missing args.upstream (string \"host:port\")".to_string()))?;
	if upstream.is_empty() {
		return Err(FactoryError("args.upstream must not be empty".to_string()));
	}
	Ok(FetchInst::L4(Arc::new(L4ForwardFetch { upstream: upstream.to_string() })))
}

/// Convenience: register this fetch against `FetchKind::L4Forward` on a
/// `FetchFactories`. A future `register_builtin_fetches` aggregator will
/// fan out to this and the H1/H2/H3 / Synthesize factories together; for
/// now this is the explicit per-fetch registration path.
pub fn register(factories: &mut FetchFactories) {
	factories.register(FetchKind::L4Forward, factory);
}
