/* src/modules/stack/protocol/application/container.rs */

use crate::common::{
	getenv,
	requirements::{Error, Result},
};
use crate::modules::{kv::KvStore, stack::protocol::application::http::wrapper::VaneBody};
use bytes::Bytes;
use http::Response;
use http_body_util::BodyExt;
use std::fmt;
use tokio::sync::oneshot;

/// Represents the payload of an L7 envelope.
/// It abstracts over HTTP bodies (H1/H2/H3) or buffered data using VaneBody.
pub enum PayloadState {
	/// A Vane-compatible HTTP Body stream (for H1/H2/H3).
	Http(VaneBody),

	/// A generic L7 stream (e.g., for future Redis/MySQL support).
	#[allow(dead_code)]
	Generic,

	/// The payload has been fully buffered into memory.
	Buffered(Bytes),

	/// No payload exists or it has been consumed.
	Empty,
}

impl fmt::Debug for PayloadState {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			PayloadState::Http(_) => write!(f, "Payload::Http(VaneBody)"),
			PayloadState::Generic => write!(f, "Payload::Generic(Stream)"),
			PayloadState::Buffered(b) => write!(f, "Payload::Buffered({} bytes)", b.len()),
			PayloadState::Empty => write!(f, "Payload::Empty"),
		}
	}
}

impl PayloadState {
	/// Internal helper to buffer the current state into memory.
	async fn force_buffer(&mut self) -> Result<&Bytes> {
		let max_len_str = getenv::get_env("L7_MAX_BUFFER_SIZE", "10485760".to_string()); // Default 10MB
		let max_len = max_len_str.parse::<usize>().unwrap_or(10485760);

		// Temporarily take ownership of self to perform transition
		let current_state = std::mem::replace(self, PayloadState::Empty);

		match current_state {
			PayloadState::Http(body) => {
				let collected = body
					.collect()
					.await
					.map_err(|e| Error::System(format!("Failed to buffer Vane body: {}", e)))?;

				let bytes = collected.to_bytes();
				if bytes.len() > max_len {
					return Err(Error::System(format!(
						"Payload too large to buffer: {} > {}",
						bytes.len(),
						max_len
					)));
				}

				*self = PayloadState::Buffered(bytes);
			}
			PayloadState::Buffered(bytes) => {
				*self = PayloadState::Buffered(bytes);
			}
			PayloadState::Generic => {
				*self = PayloadState::Buffered(Bytes::new());
			}
			PayloadState::Empty => {
				*self = PayloadState::Buffered(Bytes::new());
			}
		}

		match self {
			PayloadState::Buffered(b) => Ok(b),
			_ => unreachable!("Payload must be buffered after force_buffer logic"),
		}
	}
}

/// The Universal L7 Container (The Envelope).
pub struct Container {
	/// Metadata Store (Headers, Attributes, Routing info)
	pub kv: KvStore,

	/// The Request Body (From Client).
	/// Populated at start. Consumed by FetchUpstream.
	pub request_body: PayloadState,

	/// The Response Body (From Upstream or Generator).
	/// Populated by FetchUpstream or Terminator. Sent to Client.
	pub response_body: PayloadState,

	/// A signaling channel to send the Final Response Headers back to the Protocol Adapter.
	pub response_tx: Option<oneshot::Sender<Response<()>>>,
}

impl Container {
	pub fn new(
		kv: KvStore,
		request_body: PayloadState,
		response_body: PayloadState,
		response_tx: Option<oneshot::Sender<Response<()>>>,
	) -> Self {
		Self {
			kv,
			request_body,
			response_body,
			response_tx,
		}
	}

	/// Triggers Lazy Buffering for the REQUEST Body.
	pub async fn force_buffer_request(&mut self) -> Result<&Bytes> {
		self.request_body.force_buffer().await
	}

	/// Triggers Lazy Buffering for the RESPONSE Body.
	pub async fn force_buffer_response(&mut self) -> Result<&Bytes> {
		self.response_body.force_buffer().await
	}
}
