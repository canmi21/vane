//! vane management protocol: vane-specific verb schemas plus
//! re-exports of the project-agnostic NDJSON-RPC framing
//! ([`ndjson_rpc`]).
//!
//! See [`spec/crates/mgmt.md`](../../../spec/crates/mgmt.md).

pub mod verb;

pub use ndjson_rpc::{
	DispatchOutcome, EndMarker, EventStream, Handler, HttpMgmtClient, HttpServerConfig,
	HttpServerError, MgmtClientError, Request, Response, ResponseOutcome, UnixMgmtClient, WireError,
	WireErrorKind, encode_line, spawn_http_server, spawn_unix_server,
};
pub use ndjson_rpc::{client, http_client, http_server, protocol, server};
