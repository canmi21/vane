use vane_transport::tls::{parse_client_hello, sanitize_sni};

use crate::flow::context::ExecutionContext;
use crate::flow::plugin::{BranchAction, Middleware};

/// Middleware that parses a TLS `ClientHello` from peeked bytes and routes by SNI.
///
/// Branch name is the sanitized SNI hostname, or `"_default"` when no SNI is present.
/// Populates `tls.client.*` KV keys with parsed handshake metadata.
pub struct TlsClientHello;

impl Middleware for TlsClientHello {
	fn execute(
		&self,
		_params: &serde_json::Value,
		ctx: &dyn ExecutionContext,
	) -> Result<BranchAction, anyhow::Error> {
		let data =
			ctx.peek_data().ok_or_else(|| anyhow::anyhow!("no peek data for ClientHello parsing"))?;

		let info = parse_client_hello(data)?;

		let branch = info.sni.as_ref().map_or_else(|| "_default".to_owned(), |sni| sanitize_sni(sni));

		let mut updates = Vec::new();

		if let Some(sni) = &info.sni {
			updates.push(("tls.client.sni".to_owned(), sni.clone()));
		}
		if !info.alpn.is_empty() {
			updates.push(("tls.client.alpn".to_owned(), info.alpn.join(",")));
		}
		if !info.supported_versions.is_empty() {
			let versions: Vec<String> =
				info.supported_versions.iter().map(|v| format!("0x{v:04x}")).collect();
			updates.push(("tls.client.versions".to_owned(), versions.join(",")));
		}
		if !info.cipher_suites.is_empty() {
			let ciphers: Vec<String> = info.cipher_suites.iter().map(|v| format!("0x{v:04x}")).collect();
			updates.push(("tls.client.ciphers".to_owned(), ciphers.join(",")));
		}
		if !info.supported_groups.is_empty() {
			let groups: Vec<String> =
				info.supported_groups.iter().map(|v| format!("0x{v:04x}")).collect();
			updates.push(("tls.client.groups".to_owned(), groups.join(",")));
		}

		Ok(BranchAction { branch, updates })
	}
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
	use super::*;
	use std::net::{IpAddr, Ipv4Addr, SocketAddr};
	use vane_primitives::kv::KvStore;
	use vane_transport::stream::ConnectionStream;

	struct MockContext {
		peer: SocketAddr,
		server: SocketAddr,
		kv: KvStore,
		peek: Option<Vec<u8>>,
	}

	impl MockContext {
		fn with_peek(data: &[u8]) -> Self {
			let peer = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 1234);
			let server = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080);
			let kv = KvStore::new(&peer, &server, "tcp");
			Self { peer, server, kv, peek: Some(data.to_vec()) }
		}

		fn without_peek() -> Self {
			let peer = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 1234);
			let server = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080);
			let kv = KvStore::new(&peer, &server, "tcp");
			Self { peer, server, kv, peek: None }
		}
	}

	impl ExecutionContext for MockContext {
		fn peer_addr(&self) -> SocketAddr {
			self.peer
		}
		fn server_addr(&self) -> SocketAddr {
			self.server
		}
		fn kv(&self) -> &KvStore {
			&self.kv
		}
		fn kv_mut(&mut self) -> &mut KvStore {
			&mut self.kv
		}
		fn take_stream(&mut self) -> Option<ConnectionStream> {
			None
		}
		fn peek_data(&self) -> Option<&[u8]> {
			self.peek.as_deref()
		}
	}

	/// Build a minimal `ClientHello` with optional SNI.
	fn build_minimal_clienthello(sni: Option<&str>) -> Vec<u8> {
		let mut body = Vec::new();
		body.extend_from_slice(&[0x03, 0x03]); // version
		body.extend_from_slice(&[0u8; 32]); // random
		body.push(0x00); // session_id length
		body.extend_from_slice(&[0x00, 0x02]); // cipher suites length
		body.extend_from_slice(&[0x13, 0x01]); // TLS_AES_128_GCM_SHA256
		body.push(0x01); // compression length
		body.push(0x00); // null compression

		if let Some(hostname) = sni {
			let name = hostname.as_bytes();
			let mut ext = Vec::new();
			// SNI extension
			ext.extend_from_slice(&[0x00, 0x00]);
			let sni_list_len = 3 + name.len();
			let ext_data_len = 2 + sni_list_len;
			ext.extend_from_slice(&(ext_data_len as u16).to_be_bytes());
			ext.extend_from_slice(&(sni_list_len as u16).to_be_bytes());
			ext.push(0x00); // host_name
			ext.extend_from_slice(&(name.len() as u16).to_be_bytes());
			ext.extend_from_slice(name);

			body.extend_from_slice(&(ext.len() as u16).to_be_bytes());
			body.extend(ext);
		}

		let mut data = Vec::new();
		data.push(0x16);
		data.extend_from_slice(&[0x03, 0x01]);
		let record_len = 4 + body.len();
		data.extend_from_slice(&(record_len as u16).to_be_bytes());
		data.push(0x01); // ClientHello
		let hs_len = body.len();
		data.push((hs_len >> 16) as u8);
		data.push((hs_len >> 8) as u8);
		data.push(hs_len as u8);
		data.extend(body);
		data
	}

	#[test]
	fn valid_clienthello_branch_and_kv() {
		let data = build_minimal_clienthello(Some("example.com"));
		let ctx = MockContext::with_peek(&data);
		let plugin = TlsClientHello;

		let action = plugin.execute(&serde_json::Value::Null, &ctx).unwrap();

		assert_eq!(action.branch, "example.com");
		assert!(action.updates.iter().any(|(k, v)| k == "tls.client.sni" && v == "example.com"));
		assert!(action.updates.iter().any(|(k, _)| k == "tls.client.ciphers"));
	}

	#[test]
	fn no_sni_returns_default_branch() {
		let data = build_minimal_clienthello(None);
		let ctx = MockContext::with_peek(&data);
		let plugin = TlsClientHello;

		let action = plugin.execute(&serde_json::Value::Null, &ctx).unwrap();
		assert_eq!(action.branch, "_default");
		assert!(!action.updates.iter().any(|(k, _)| k == "tls.client.sni"));
	}

	#[test]
	fn no_peek_data_returns_error() {
		let ctx = MockContext::without_peek();
		let plugin = TlsClientHello;
		assert!(plugin.execute(&serde_json::Value::Null, &ctx).is_err());
	}

	#[test]
	fn non_tls_data_returns_error() {
		let ctx = MockContext::with_peek(b"GET / HTTP/1.1\r\n");
		let plugin = TlsClientHello;
		assert!(plugin.execute(&serde_json::Value::Null, &ctx).is_err());
	}
}
