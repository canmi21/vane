/* src/modules/stack/application/http/protocol_data.rs */

use std::any::Any;

use hyper::upgrade::OnUpgrade;

use crate::modules::stack::application::protocol_data::ProtocolData;

/// HTTP-specific protocol extension data.
///
/// This structure stores HTTP/1.1 WebSocket upgrade handles that are not
/// applicable to other L7 protocols (DNS, gRPC, etc.).
///
/// # Fields
///
/// - `client_upgrade`: WebSocket upgrade handle from the client connection.
///   Populated by the HTTP adapter when detecting an Upgrade request.
///   Consumed by the Response terminator to establish a bidirectional tunnel.
///
/// - `upstream_upgrade`: WebSocket upgrade handle from the upstream server.
///   Populated by FetchUpstream when the backend responds with HTTP 101.
///   Consumed by the Response terminator to establish a bidirectional tunnel.
///
/// # Design Note
///
/// This data is only relevant for HTTP/1.1 connections with WebSocket upgrades.
/// HTTP/2 and HTTP/3 do not use this mechanism (they have native bidirectional streams).
#[derive(Default)]
pub struct HttpProtocolData {
	/// Client-side WebSocket Upgrade Handle (HTTP/1.1 only).
	pub client_upgrade: Option<OnUpgrade>,

	/// Upstream-side WebSocket Upgrade Handle (HTTP/1.1 only).
	pub upstream_upgrade: Option<OnUpgrade>,
}

impl HttpProtocolData {
	/// Creates a new HttpProtocolData with no upgrade handles.
	pub fn new() -> Self {
		Self {
			client_upgrade: None,
			upstream_upgrade: None,
		}
	}
}

impl ProtocolData for HttpProtocolData {
	fn as_any(&self) -> &dyn Any {
		self
	}

	fn as_any_mut(&mut self) -> &mut dyn Any {
		self
	}
}
