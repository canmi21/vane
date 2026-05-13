//! Line-delimited JSON-RPC framing (`{ id, verb, args }` ↔
//! `{ id, result | error | event | end }`) with two interchangeable
//! transports — a Unix-domain-socket server / client for local
//! control planes, and an HTTP/1.1 server / client that streams the
//! same frames as `Transfer-Encoding: chunked` NDJSON.
//!
//! See the README for the gap this fills (daemon ↔ CLI control planes
//! that everyone re-implements).

pub mod client;
pub mod http_client;
pub mod http_server;
pub mod protocol;
pub mod server;

pub use client::{ClientError, UnixClient};
pub use http_client::HttpClient;
pub use http_server::{HttpServerConfig, HttpServerError, spawn_http_server};
pub use protocol::{
	EndMarker, Request, Response, ResponseOutcome, WireError, WireErrorKind, encode_line,
};
pub use server::{DispatchOutcome, EventStream, Handler, spawn_unix_server};
