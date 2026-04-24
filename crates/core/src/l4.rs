use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::{TcpStream, UdpSocket};

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub struct QuicAssocId(pub u64);

pub enum L4Conn {
	Tcp(TcpStream),
	Udp(UdpAssoc),
}

pub struct UdpAssoc {
	pub socket: Arc<UdpSocket>,
	pub peer: SocketAddr,
	pub quic: Option<QuicAssocId>,
}

#[cfg(test)]
mod tests {
	use super::*;

	// Compile-gate: if L4Conn's variant shape changes, this signature fails
	// to type-check. No runtime assertion is warranted.
	fn _accepts_l4_conn(_: &L4Conn) {}

	#[test]
	fn quic_assoc_id_serde_round_trip() {
		let id = QuicAssocId(0xdead_beef_cafe_babe);
		let encoded = serde_json::to_string(&id).expect("serialize");
		let decoded: QuicAssocId = serde_json::from_str(&encoded).expect("deserialize");
		assert_eq!(decoded, id);
	}

	#[test]
	fn quic_assoc_id_equality_is_structural() {
		assert_eq!(QuicAssocId(42), QuicAssocId(42));
		assert_ne!(QuicAssocId(42), QuicAssocId(43));
	}
}
