/* src/modules/stack/protocol/application/container.rs */

use crate::common::{
	getenv,
	requirements::{Error, Result},
};
use crate::modules::kv::KvStore;
use bytes::Bytes;
use http_body_util::{BodyExt, combinators::BoxBody};
use std::fmt;

/// Represents the payload of an L7 envelope.
/// It abstracts over HTTP bodies, generic streams, or buffered data.
pub enum PayloadState {
	/// A Hyper-compatible HTTP Body stream (for H1/H2/H3).
	Http(BoxBody<Bytes, hyper::Error>),

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
			PayloadState::Http(_) => write!(f, "Payload::Http(Stream)"),
			PayloadState::Generic => write!(f, "Payload::Generic(Stream)"),
			PayloadState::Buffered(b) => write!(f, "Payload::Buffered({} bytes)", b.len()),
			PayloadState::Empty => write!(f, "Payload::Empty"),
		}
	}
}

/// The Universal L7 Container (The Envelope).
pub struct Container {
	/// Metadata Store (Headers, Attributes, Routing info)
	pub kv: KvStore,

	/// The primary payload (Request Body or Upstream Message)
	pub payload: PayloadState,
}

impl Container {
	pub fn new(kv: KvStore, payload: PayloadState) -> Self {
		Self { kv, payload }
	}

	/// Triggers the "Lazy Buffer" mechanism.
	pub async fn force_buffer(&mut self) -> Result<&Bytes> {
		let max_len_str = getenv::get_env("L7_MAX_BUFFER_SIZE", "10485760".to_string()); // Default 10MB
		let max_len = max_len_str.parse::<usize>().unwrap_or(10485760);

		let current_state = std::mem::replace(&mut self.payload, PayloadState::Empty);

		match current_state {
			PayloadState::Http(body) => {
				let collected = body
					.collect()
					.await
					.map_err(|e| Error::Tls(format!("Failed to buffer HTTP body: {}", e)))?;

				let bytes = collected.to_bytes();
				if bytes.len() > max_len {
					return Err(Error::System(format!(
						"Payload too large to buffer: {} > {}",
						bytes.len(),
						max_len
					)));
				}

				self.payload = PayloadState::Buffered(bytes);
			}
			PayloadState::Buffered(bytes) => {
				self.payload = PayloadState::Buffered(bytes);
			}
			PayloadState::Generic => {
				self.payload = PayloadState::Buffered(Bytes::new());
			}
			PayloadState::Empty => {
				self.payload = PayloadState::Buffered(Bytes::new());
			}
		}

		match &self.payload {
			PayloadState::Buffered(b) => Ok(b),
			_ => unreachable!("Payload must be buffered after force_buffer"),
		}
	}
}
