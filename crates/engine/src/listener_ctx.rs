//! Listener-wide and per-connection context bags. Single Arc-shared
//! [`AcceptCtx`] per spawned listener carries the lifecycle state every
//! handler needs (graph swap, verbosity, log sink, security, in-flight
//! tracking, cancel tokens, bind config, conn registry). UDP listeners
//! extend it with [`UdpAcceptCtx`] for the physical socket + dispatch
//! table; per-connection dispatch helpers receive a small
//! [`ConnDispatchCtx`] carrying the captured graph snapshot and the
//! resolved per-accept state.
//!
//! These bags exist to keep listener / handler signatures honest: every
//! function used to take 8-14 individual `Arc<T>` params, all repeating
//! the same lifecycle wiring at every call site. Packing them here lets
//! each function declare what it actually needs (listener-wide vs
//! per-conn) and lets `tokio::spawn` clone a single `Arc` instead of N.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};

use arc_swap::ArcSwap;
use dashmap::DashMap;
use in_flight_set::InFlightSet;

use tokio_util::sync::CancellationToken;
use vane_core::{ConnContext, ConnId, FlowLogSink, ListenerKind, NodeId};

use crate::flow_graph::FlowGraph;
use crate::listener::{BindConfig, ConnEntry};
use crate::security::SecurityState;
use crate::verbosity::VerbosityState;

/// State shared by a single listener's accept loop and every per-connection
/// task it spawns. One [`Arc<AcceptCtx>`] per spawned listener; cloned
/// (refcount-only) into each spawned handler.
pub(crate) struct AcceptCtx {
	pub addr: SocketAddr,
	pub graph: Arc<ArcSwap<FlowGraph>>,
	pub verbosity: Arc<VerbosityState>,
	pub log_sink: Arc<dyn FlowLogSink>,
	pub security: Arc<SecurityState>,
	pub accept_cancel: CancellationToken,
	pub force_cancel: CancellationToken,
	/// Per-listener supervised task set. See
	/// [`in_flight_set::InFlightSet`] — the wrapper enforces the
	/// "take under brief sync critical section, then `join_next`
	/// off-lock" invariant so drain code paths never hold a sync
	/// mutex across `.await`.
	pub in_flight: Arc<InFlightSet>,
	pub in_flight_count: Arc<AtomicUsize>,
	pub bind_ready: Arc<AtomicBool>,
	pub bind_cfg: Arc<BindConfig>,
	pub connections: Arc<DashMap<ConnId, ConnEntry>>,
}

/// UDP listener extension: adds the physical socket + per-listener
/// dispatch table. The TCP path doesn't need either; carrying them
/// optionally on `AcceptCtx` would lie about the schema.
pub(crate) struct UdpAcceptCtx {
	pub base: Arc<AcceptCtx>,
	pub socket: Arc<tokio::net::UdpSocket>,
	pub dispatch_table: Arc<crate::listener_udp::DispatchTable>,
}

/// Per-connection dispatch state shared by `dispatch_no_peek` and
/// `dispatch_peeked`. Built once inside `handle_connection` after the
/// graph snapshot + `ConnContext` are in hand.
pub(crate) struct ConnDispatchCtx {
	pub kind: ListenerKind,
	/// Captured `FlowGraph` snapshot; not the swap. Reload cannot pull
	/// the rug on this connection.
	pub graph: Arc<FlowGraph>,
	pub entry: NodeId,
	pub conn: Arc<ConnContext>,
	pub remote: SocketAddr,
	/// Listener bind address — same as `AcceptCtx.addr`, copied here so
	/// dispatch can query `FlowGraph` accessors keyed by listener
	/// address (e.g. `declares_tls`) without re-deriving from `conn`.
	pub local_addr: SocketAddr,
	pub tls_cfg: Option<Arc<rustls::ServerConfig>>,
}
