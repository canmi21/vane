/* src/transport/src/protocol/quic/mod.rs */

pub mod crypto;
pub mod frame;
pub mod packet;

// Re-export types to keep existing code working
pub mod parser {
	pub use super::frame::parse_tls_client_hello_sni;
	pub use super::packet::{
		QuicInitialData, parse_initial_packet, peek_long_header_dcid, peek_short_header_dcid,
		read_varint,
	};
	// Note: 'extract_sni_from_initial' is now internal to packet parsing logic,
	// but exposed if needed for advanced usage.
	pub use super::crypto::extract_decrypted_content;
}
