/* src/modules/stack/application/container.rs */

use std::sync::atomic::{AtomicUsize, Ordering};

use bytes::Bytes;
use fancy_log::{LogLevel, log};
use http::{HeaderMap, Response};
use http_body::Body;
use http_body_util::BodyExt;
use hyper::upgrade::OnUpgrade;
use tokio::sync::oneshot;

use crate::common::{
	getenv,
	lifecycle::{Error, Result},
};
use crate::modules::{
	kv::KvStore,
	stack::application::{
		http::{protocol_data::HttpProtocolData, wrapper::VaneBody},
		protocol_data::ProtocolData,
	},
};

// --- Memory Management State ---

/// Global counter for total buffered L7 bytes.
pub static GLOBAL_L7_BUFFERED_BYTES: AtomicUsize = AtomicUsize::new(0);

/// Current memory limit (dynamic or fixed).
pub static CURRENT_MEMORY_LIMIT: AtomicUsize = AtomicUsize::new(536_870_912); // Default 512MB

/// Updates the dynamic memory limit.
pub fn update_memory_limit(new_limit: usize) {
	CURRENT_MEMORY_LIMIT.store(new_limit, Ordering::Relaxed);
}

/// Tries to reserve memory for buffering. Returns true if successful.
pub fn try_reserve_buffer_memory(amount: usize) -> bool {
	let limit = CURRENT_MEMORY_LIMIT.load(Ordering::Relaxed);
	let current = GLOBAL_L7_BUFFERED_BYTES.load(Ordering::Relaxed);
	if current + amount > limit {
		log(
			LogLevel::Warn,
			&format!(
				"🛡 Security: L7 Global Buffer Limit reached! Denying allocation of {} bytes (Used: {}/{})",
				amount, current, limit
			),
		);
		return false;
	}
	GLOBAL_L7_BUFFERED_BYTES.fetch_add(amount, Ordering::Relaxed);
	true
}

/// Releases memory from the global counter.
pub fn release_buffer_memory(amount: usize) {
	GLOBAL_L7_BUFFERED_BYTES.fetch_sub(amount, Ordering::Relaxed);
}

/// A RAII guard that tracks buffered L7 memory.
/// When dropped, it automatically releases its quota from the global counter.
#[derive(Debug)]
pub struct BufferGuard {
	size: usize,
}

impl BufferGuard {
	pub fn new(size: usize) -> Self {
		Self { size }
	}
}

impl Drop for BufferGuard {
	fn drop(&mut self) {
		if self.size > 0 {
			release_buffer_memory(self.size);
		}
	}
}

/// Represents the payload of an L7 envelope.
/// It abstracts over HTTP bodies (H1/H2/H3) or buffered data using VaneBody.
#[derive(Debug)]
pub enum PayloadState {
	/// A Vane-compatible HTTP Body stream (for H1/H2/H3).
	Http(VaneBody),

	/// A generic L7 stream (e.g., for future Redis/MySQL support).
	Generic,

	/// The payload has been fully buffered into memory.
	/// The BufferGuard ensures the global quota is released when this state is dropped.
	Buffered(Bytes, BufferGuard),

	/// No payload exists or it has been consumed.
	Empty,
}

impl PayloadState {
	/// Creates a new Buffered payload state, enforcing global memory limits.
	pub fn new_buffered(bytes: Bytes) -> Result<Self> {
		let len = bytes.len();
		if !try_reserve_buffer_memory(len) {
			return Err(Error::System(
				"Global L7 memory limit exceeded. Buffering denied.".to_string(),
			));
		}
		Ok(PayloadState::Buffered(bytes, BufferGuard::new(len)))
	}

	/// Internal helper to buffer the current state into memory.
	async fn force_buffer(&mut self) -> Result<&Bytes> {
		let max_len_str = getenv::get_env("L7_MAX_BUFFER_SIZE", "10485760".to_string()); // Default 10MB
		let max_len = max_len_str.parse::<usize>().unwrap_or(10485760);

		// Temporarily take ownership of self to perform transition
		// We use Empty as a placeholder during the async collect()
		let current_state = std::mem::replace(self, PayloadState::Empty);

		match current_state {
			PayloadState::Http(body) => {
				// 1. Get body size hint if available
				let size_hint = body.size_hint().lower() as usize;
				if size_hint > max_len {
					*self = PayloadState::Http(body); // Restore
					return Err(Error::System(format!(
						"Payload size hint too large: {} > {}",
						size_hint, max_len
					)));
				}

				// 2. Perform the collection
				let collected = body
					.collect()
					.await
					.map_err(|e| Error::System(format!("Failed to buffer Vane body: {}", e)))?;

				let bytes = collected.to_bytes();
				let actual_len = bytes.len();

				if actual_len > max_len {
					return Err(Error::System(format!(
						"Actual payload too large to buffer: {} > {}",
						actual_len, max_len
					)));
				}

				// 3. Create buffered state (enforces GLOBAL quota)
				*self = Self::new_buffered(bytes)?;
			}
			PayloadState::Buffered(bytes, guard) => {
				// Already buffered, just restore
				*self = PayloadState::Buffered(bytes, guard);
			}
			PayloadState::Generic => {
				*self = Self::new_buffered(Bytes::new())?;
			}
			PayloadState::Empty => {
				*self = PayloadState::Empty;
			}
		}

		match self {
			PayloadState::Buffered(b, _) => Ok(b),
			_ => Err(Error::System(
				"Internal state inconsistency: payload not buffered after force_buffer".to_string(),
			)),
		}
	}
}

/// The Universal L7 Container (The Envelope).
///
/// # Architecture Note (Hybrid Storage)
/// - **KV (Control Plane):** Stores high-freq metadata (IP, Method, Path) for routing.
/// - **Headers/Body (Data Plane):** Stores the full protocol payload.
///   Accessed via "Magic Words" in the Template System (On-Demand Copy).
/// - **Protocol Data (Extension Plane):** Protocol-specific extension fields.
///   HTTP uses this for WebSocket upgrade handles. Future protocols (DNS, gRPC)
///   can inject their own data without polluting the core structure.
pub struct Container {
	/// Metadata Store (Control Plane)
	pub kv: KvStore,

	/// The Request Headers (Data Plane).
	/// Populated by Adapter. Hijacked by Template System. Consumed by Upstream.
	pub request_headers: HeaderMap,

	/// The Request Body (Data Plane).
	/// Populated at start. Hijacked by Template System (Lazy Buffer). Consumed by FetchUpstream.
	pub request_body: PayloadState,

	/// The Response Headers (Data Plane).
	/// Populated by Upstream/Terminator. Sent to Client.
	pub response_headers: HeaderMap,

	/// The Response Body (Data Plane).
	/// Populated by FetchUpstream or Terminator. Sent to Client.
	pub response_body: PayloadState,

	/// A signaling channel to send the Final Response Headers back to the Protocol Adapter.
	pub response_tx: Option<oneshot::Sender<Response<()>>>,

	/// Protocol-specific extension data (HTTP, DNS, gRPC, etc.).
	/// Use `http_data()` / `http_data_mut()` helpers to access HTTP-specific fields.
	pub protocol_data: Option<Box<dyn ProtocolData>>,
}

impl Container {
	/// Creates a new Container with no protocol-specific data.
	pub fn new(
		kv: KvStore,
		request_headers: HeaderMap,
		request_body: PayloadState,
		response_headers: HeaderMap,
		response_body: PayloadState,
		response_tx: Option<oneshot::Sender<Response<()>>>,
	) -> Self {
		Self {
			kv,
			request_headers,
			request_body,
			response_headers,
			response_body,
			response_tx,
			protocol_data: None,
		}
	}

	/// Creates a new Container with HTTP protocol data (for WebSocket support).
	pub fn new_with_http(
		kv: KvStore,
		request_headers: HeaderMap,
		request_body: PayloadState,
		response_headers: HeaderMap,
		response_body: PayloadState,
		response_tx: Option<oneshot::Sender<Response<()>>>,
	) -> Self {
		let mut container = Self::new(
			kv,
			request_headers,
			request_body,
			response_headers,
			response_body,
			response_tx,
		);
		container.protocol_data = Some(Box::new(HttpProtocolData::new()));
		container
	}

	/// Gets a reference to HTTP protocol data (if present).
	///
	/// Returns None if Container was not created with HTTP protocol support.
	pub fn http_data(&self) -> Option<&HttpProtocolData> {
		self
			.protocol_data
			.as_ref()?
			.as_any()
			.downcast_ref::<HttpProtocolData>()
	}

	/// Gets a mutable reference to HTTP protocol data (if present).
	///
	/// Returns None if Container was not created with HTTP protocol support.
	pub fn http_data_mut(&mut self) -> Option<&mut HttpProtocolData> {
		self
			.protocol_data
			.as_mut()?
			.as_any_mut()
			.downcast_mut::<HttpProtocolData>()
	}

	/// Deprecated: Access via `container.http_data()?.client_upgrade` instead.
	///
	/// Gets the client-side WebSocket upgrade handle.
	#[deprecated(
		since = "0.6.9",
		note = "Use container.http_data()?.client_upgrade to access this field"
	)]
	pub fn get_client_upgrade(&self) -> Option<&OnUpgrade> {
		self.http_data()?.client_upgrade.as_ref()
	}

	/// Deprecated: Access via `container.http_data_mut()?.client_upgrade = Some(...)` instead.
	///
	/// Sets the client-side WebSocket upgrade handle.
	#[deprecated(
		since = "0.6.9",
		note = "Use container.http_data_mut()?.client_upgrade = Some(...) to set this field"
	)]
	pub fn set_client_upgrade(&mut self, upgrade: OnUpgrade) {
		if let Some(data) = self.http_data_mut() {
			data.client_upgrade = Some(upgrade);
		}
	}

	/// Deprecated: Access via `container.http_data()?.upstream_upgrade` instead.
	///
	/// Gets the upstream-side WebSocket upgrade handle.
	#[deprecated(
		since = "0.6.9",
		note = "Use container.http_data()?.upstream_upgrade to access this field"
	)]
	pub fn get_upstream_upgrade(&self) -> Option<&OnUpgrade> {
		self.http_data()?.upstream_upgrade.as_ref()
	}

	/// Deprecated: Access via `container.http_data_mut()?.upstream_upgrade = Some(...)` instead.
	///
	/// Sets the upstream-side WebSocket upgrade handle.
	#[deprecated(
		since = "0.6.9",
		note = "Use container.http_data_mut()?.upstream_upgrade = Some(...) to set this field"
	)]
	pub fn set_upstream_upgrade(&mut self, upgrade: OnUpgrade) {
		if let Some(data) = self.http_data_mut() {
			data.upstream_upgrade = Some(upgrade);
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
