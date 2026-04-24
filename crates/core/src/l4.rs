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
