use std::net::SocketAddr;
use std::sync::OnceLock;
use std::time::Instant;

use parking_lot::Mutex;
use rustls_pki_types::CertificateDer;

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub struct ConnId(pub u64);

impl std::fmt::Display for ConnId {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "{:016x}", self.0)
	}
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub enum Transport {
	Tcp,
	Udp,
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub enum HttpVersion {
	Http1_0,
	Http1_1,
	Http2,
	Http3,
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub enum TlsVersion {
	Tls12,
	Tls13,
}

#[derive(Clone, Debug)]
pub struct TlsInfo {
	pub sni: Option<String>,
	pub alpn: Option<Vec<u8>>,
	pub version: Option<TlsVersion>,
	pub peer_cert: Option<CertificateDer<'static>>,
}

pub struct ConnContext {
	pub id: ConnId,
	pub remote: SocketAddr,
	pub local: SocketAddr,
	pub transport: Transport,
	pub entered_at: Instant,

	pub tls: Mutex<Option<TlsInfo>>,
	pub http_version: OnceLock<HttpVersion>,

	pub user: Mutex<http::Extensions>,
}
