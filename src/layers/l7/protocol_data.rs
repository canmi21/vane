/* src/layers/l7/protocol_data.rs */

use std::any::Any;

/// Trait for protocol-specific data stored in Container.
///
/// This abstraction allows different L7 protocols (HTTP, DNS, gRPC) to extend
/// Container with protocol-specific fields without polluting the core structure.
///
/// # Design Rationale
///
/// The Container struct provides a generic envelope for L7 data (headers, body, KV).
/// However, different protocols need protocol-specific extension points:
/// - HTTP: WebSocket upgrade handles (OnUpgrade)
/// - DNS: Query ID, response code
/// - gRPC: Stream metadata, trailers
///
/// Instead of adding all protocol-specific fields to Container, we use this trait
/// to allow protocols to inject their own data via `Container.protocol_data`.
///
/// # Type Erasure
///
/// This trait uses type erasure (`Box<dyn ProtocolData>`) to maintain compatibility
/// with the current plugin system, which passes `&mut (dyn Any + Send)` to plugins.
pub trait ProtocolData: Send + Sync {
	/// Downcast helper for accessing concrete protocol data.
	fn as_any(&self) -> &dyn Any;

	/// Mutable downcast helper for modifying protocol data.
	fn as_any_mut(&mut self) -> &mut dyn Any;
}
