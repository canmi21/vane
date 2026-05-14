//! HTTP/3 listener-side glue: per-listener QUIC endpoint built on top
//! of a [`virtual_socket::VirtualUdpSocket`] (so vane can keep
//! demultiplexing the physical UDP socket between QUIC and other
//! traffic kinds), wrapped in [`quinn_shared_socket::SharedSocket`]
//! to satisfy [`quinn::AsyncUdpSocket`]. The endpoint is configured
//! with the daemon's [`crate::tls::VaneCertResolver`] for ALPN `h3`.
//!
//! See `spec/crates/engine.md` § _`udp_dispatch`_, and `spec/crates/engine-tls.md` § _Cert resolver_. The whole module is gated behind the `h3` cargo feature.

use std::net::SocketAddr;
use std::sync::Arc;

use arc_swap::ArcSwap;
use in_flight_set::InFlightSet;
use quinn_shared_socket::SharedSocket;
use tokio_util::sync::CancellationToken;
use vane_core::FlowLogSink;
use virtual_socket::VirtualUdpSocket;

use crate::flow_graph::FlowGraph;
use crate::listener_ctx::UdpAcceptCtx;
use crate::listener_udp::{DispatchHandle, DispatchKey};
use crate::verbosity::VerbosityState;

/// Build the per-listener `quinn::ServerConfig` for ALPN `h3`. Reuses
/// the daemon's `Arc<rustls::ServerConfig>` (whose cert resolver is the
/// shared `VaneCertResolver`); only the ALPN list is overridden to
/// `[b"h3"]` per RFC 9114, and `enable_zero_rtt` is left at its rustls
/// default (false).
///
/// # Errors
///
/// Surfaces any `quinn::crypto::rustls` build error as a string.
pub fn build_quic_server_config(
	rustls_cfg: &Arc<rustls::ServerConfig>,
) -> Result<quinn::ServerConfig, String> {
	// Clone the rustls config and override ALPN to h3 only — H3 ALPN
	// is `h3` (RFC 9114). The original rustls config (used by the TCP
	// listener) keeps its h2/http1.1 ALPN unchanged via Arc-share.
	let inner: rustls::ServerConfig = (**rustls_cfg).clone();
	let mut h3_rustls = inner;
	h3_rustls.alpn_protocols = vec![b"h3".to_vec()];
	// TODO(0rtt-h3): TLS 1.3 0-RTT for H3 is deferred — leave
	// `enable_zero_rtt` / `max_early_data_size` at rustls defaults
	// until h3-quinn surfaces a stable per-stream 0-RTT signal.
	let h3_rustls = Arc::new(h3_rustls);

	let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(h3_rustls)
		.map_err(|e| format!("quic server config: {e}"))?;
	Ok(quinn::ServerConfig::with_crypto(Arc::new(crypto)))
}

/// Bring up the H3 stack on a UDP listener whose derived
/// [`vane_core::ListenerKind`] is `Http`. Builds the `quinn::Endpoint`
/// against a [`VirtualUdpSocket`] wrapping the listener's physical
/// socket, registers the virtual socket in the dispatch table under
/// the well-known `QuicConnId(empty)` slot — the per-listener model
/// spec'd in `spec/crates/engine.md` § _`udp_dispatch`_ holds exactly one virtual socket per `Http` UDP listener,
/// so the empty-CID key is the listener's single QUIC fan-in slot
/// rather than a per-connection key — then spawns the accept loop
/// that hands each new connection to `drive_h3_server`.
///
/// `tls_cfg` is the same `Arc<rustls::ServerConfig>` the TCP path
/// uses (cert resolver = `VaneCertResolver`); only ALPN is overridden
/// to `[b"h3"]` per RFC 9114.
///
/// # Errors
///
/// Returns a stringly error if the QUIC server config or the
/// `quinn::Endpoint` fails to construct.
pub(crate) fn spawn_h3_endpoint(
	ctx: &Arc<UdpAcceptCtx>,
	tls_cfg: &Arc<rustls::ServerConfig>,
) -> Result<(), String> {
	let server_config = build_quic_server_config(tls_cfg)?;

	let virtual_socket: Arc<VirtualUdpSocket> = VirtualUdpSocket::new(Arc::clone(&ctx.socket));
	ctx.dispatch_table.insert(
		DispatchKey::QuicConnId(quinn_proto::ConnectionId::new(&[])),
		Arc::new(DispatchHandle::Quic(Arc::clone(&virtual_socket))),
	);

	let runtime = Arc::new(quinn::TokioRuntime);
	let endpoint = quinn::Endpoint::new_with_abstract_socket(
		quinn::EndpointConfig::default(),
		Some(server_config),
		SharedSocket::new(virtual_socket),
		runtime,
	)
	.map_err(|e| format!("quic endpoint: {e}"))?;

	let addr = ctx.base.addr;
	let graph = Arc::clone(&ctx.base.graph);
	let log_sink = Arc::clone(&ctx.base.log_sink);
	let verbosity = Arc::clone(&ctx.base.verbosity);
	let accept_cancel = ctx.base.accept_cancel.clone();
	let force_cancel = ctx.base.force_cancel.clone();
	let in_flight = Arc::clone(&ctx.base.in_flight);
	tokio::spawn(async move {
		run_h3_accept_loop(
			addr,
			endpoint,
			graph,
			&log_sink,
			&verbosity,
			accept_cancel,
			force_cancel,
			in_flight,
		)
		.await;
	});
	Ok(())
}

/// Accept-loop task: pulls each `Incoming` from the endpoint, fully
/// negotiates the QUIC handshake, then spawns
/// [`crate::upgrade::drive_h3_server`] into the listener-wide
/// `in_flight` `JoinSet` so the same shutdown tier
/// (`accept_cancel` → drain → `force_cancel` → abort) that drains TCP
/// connections also drains H3.
///
/// Two cancel tokens flow through the H3 stack:
///
/// - `accept_cancel` stops accepting new QUIC connections (this loop)
///   and, downstream, new H3 streams (the per-conn driver) — but lets
///   in-flight streams run to completion.
/// - `force_cancel` is the hard cancel — closes the endpoint and
///   propagates into per-stream `FlowCtx::cancel` for immediate teardown.
#[allow(
	clippy::too_many_arguments,
	reason = "accept-loop wiring: each arg threads one piece of listener state; alternative is a fresh bag struct that just renames the noise"
)]
async fn run_h3_accept_loop(
	addr: SocketAddr,
	endpoint: quinn::Endpoint,
	graph: Arc<ArcSwap<FlowGraph>>,
	log_sink: &Arc<dyn FlowLogSink>,
	verbosity: &Arc<VerbosityState>,
	accept_cancel: CancellationToken,
	force_cancel: CancellationToken,
	in_flight: Arc<InFlightSet>,
) {
	loop {
		tokio::select! {
			biased;
			() = force_cancel.cancelled() => {
				endpoint.close(0u32.into(), b"shutdown");
				return;
			}
			() = accept_cancel.cancelled() => {
				// Soft drain: stop accepting new QUIC connections but
				// let in-flight ones run to completion. The endpoint
				// stays open so per-conn drivers can finish their
				// streams; `force_cancel` is the kill-switch.
				tracing::debug!(?addr, "h3 accept loop received accept_cancel; stopping accept");
				return;
			}
			incoming = endpoint.accept() => {
				let Some(incoming) = incoming else {
					return; // endpoint closed
				};
				let connecting = match incoming.accept() {
					Ok(c) => c,
					Err(e) => {
						tracing::debug!(?addr, error = %e, "h3 incoming accept failed");
						continue;
					}
				};
				let graph = Arc::clone(&graph);
				let log_sink = Arc::clone(log_sink);
				let verbosity = Arc::clone(verbosity);
				let accept_cancel = accept_cancel.clone();
				let force_cancel = force_cancel.clone();
				// Spawn into the listener's `in_flight` set so the
				// per-listener drain (shutdown / reconcile) joins on
				// the H3 driver instead of leaking it as a detached
				// `tokio::spawn`.
				in_flight.spawn(async move {
					match connecting.await {
						Ok(quic_conn) => {
							crate::upgrade::drive_h3_server(
								addr,
								quic_conn,
								graph,
								log_sink,
								accept_cancel,
								force_cancel,
								verbosity,
							)
							.await;
						}
						Err(e) => {
							tracing::debug!(?addr, error = %e, "h3 quic handshake failed");
						}
					}
				});
			}
		}
	}
}
