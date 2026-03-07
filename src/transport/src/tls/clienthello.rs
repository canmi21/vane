use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientHelloInfo {
	pub sni: Option<String>,
	pub alpn: Vec<String>,
	pub supported_versions: Vec<u16>,
	pub cipher_suites: Vec<u16>,
	pub supported_groups: Vec<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ClientHelloError {
	#[error("data shorter than minimum TLS record header")]
	TooShort,
	#[error("record type is not handshake (0x16)")]
	NotHandshake,
	#[error("handshake type is not ClientHello (0x01)")]
	NotClientHello,
	#[error("length field mismatch or overflow")]
	InvalidLength,
	#[error("malformed extension data")]
	MalformedExtension,
}

/// Parse a TLS `ClientHello` from raw peeked bytes.
///
/// Truncation before the extensions boundary returns `TooShort`.
/// Truncation at or after the extensions boundary returns `Ok` with partial results.
pub fn parse_client_hello(data: &[u8]) -> Result<ClientHelloInfo, ClientHelloError> {
	let mut c = Cursor::new(data);

	// -- Record header (5 bytes) --
	let content_type = c.read_u8()?;
	if content_type != 0x16 {
		return Err(ClientHelloError::NotHandshake);
	}
	let _record_version = c.read_u16()?;
	let record_length = c.read_u16()? as usize;

	// Clamp to available data (peek buffer may be smaller than full record)
	let available = c.remaining().min(record_length);
	let record_end = c.pos + available;

	// -- Handshake header (4 bytes) --
	let hs_type = c.read_u8()?;
	if hs_type != 0x01 {
		return Err(ClientHelloError::NotClientHello);
	}
	let _hs_length = c.read_u24()?;

	// -- ClientHello body --
	let _client_version = c.read_u16()?;
	c.skip(32)?; // random

	// session_id
	let session_id_len = c.read_u8()? as usize;
	c.skip(session_id_len)?;

	// cipher_suites
	let cs_len = c.read_u16()? as usize;
	if !cs_len.is_multiple_of(2) {
		return Err(ClientHelloError::InvalidLength);
	}
	let cs_data = c.read_bytes(cs_len)?;
	let cipher_suites: Vec<u16> =
		cs_data.chunks_exact(2).map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]])).collect();

	// compression_methods
	let comp_len = c.read_u8()? as usize;
	c.skip(comp_len)?;

	// -- Extensions (optional — truncation here is OK) --
	let mut info = ClientHelloInfo {
		sni: None,
		alpn: Vec::new(),
		supported_versions: Vec::new(),
		cipher_suites,
		supported_groups: Vec::new(),
	};

	// No more data or not enough for extensions length
	if c.pos + 2 > record_end {
		return Ok(info);
	}

	let extensions_len = match c.read_u16() {
		Ok(len) => len as usize,
		Err(_) => return Ok(info),
	};

	let extensions_end = c.pos.saturating_add(extensions_len).min(record_end);

	while c.pos + 4 <= extensions_end {
		let Ok(ext_type) = c.read_u16() else { break };
		let Ok(ext_len) = c.read_u16().map(|v| v as usize) else {
			break;
		};

		if c.pos + ext_len > extensions_end {
			// Truncated extension — keep partial results
			break;
		}

		let ext_end = c.pos + ext_len;

		match ext_type {
			0x0000 => parse_sni_extension(&mut c, ext_end, &mut info)?,
			0x000a => parse_supported_groups(&mut c, ext_end, &mut info)?,
			0x0010 => parse_alpn_extension(&mut c, ext_end, &mut info)?,
			0x002b => parse_supported_versions(&mut c, ext_end, &mut info)?,
			_ => c.pos = ext_end,
		}

		c.pos = ext_end;
	}

	Ok(info)
}

/// Lowercase + keep only `[a-z0-9.\-_]`.
pub fn sanitize_sni(raw: &str) -> String {
	raw
		.to_lowercase()
		.chars()
		.filter(|ch| ch.is_ascii_alphanumeric() || *ch == '.' || *ch == '-' || *ch == '_')
		.collect()
}

// -- Extension parsers --

fn parse_sni_extension(
	c: &mut Cursor<'_>,
	ext_end: usize,
	info: &mut ClientHelloInfo,
) -> Result<(), ClientHelloError> {
	if c.pos + 2 > ext_end {
		return Err(ClientHelloError::MalformedExtension);
	}
	let _list_len = c.read_u16().map_err(|_| ClientHelloError::MalformedExtension)?;

	while c.pos + 3 <= ext_end {
		let name_type = c.read_u8().map_err(|_| ClientHelloError::MalformedExtension)?;
		let name_len = c.read_u16().map_err(|_| ClientHelloError::MalformedExtension)? as usize;

		if c.pos + name_len > ext_end {
			return Err(ClientHelloError::MalformedExtension);
		}
		let name_bytes = c.read_bytes(name_len).map_err(|_| ClientHelloError::MalformedExtension)?;

		// host_name type = 0x00
		if name_type == 0x00 {
			if let Ok(s) = std::str::from_utf8(name_bytes) {
				info.sni = Some(s.to_lowercase());
			}
			return Ok(());
		}
	}
	Ok(())
}

fn parse_alpn_extension(
	c: &mut Cursor<'_>,
	ext_end: usize,
	info: &mut ClientHelloInfo,
) -> Result<(), ClientHelloError> {
	if c.pos + 2 > ext_end {
		return Err(ClientHelloError::MalformedExtension);
	}
	let _list_len = c.read_u16().map_err(|_| ClientHelloError::MalformedExtension)?;

	while c.pos < ext_end {
		let proto_len = c.read_u8().map_err(|_| ClientHelloError::MalformedExtension)? as usize;
		if c.pos + proto_len > ext_end {
			return Err(ClientHelloError::MalformedExtension);
		}
		let proto_bytes = c.read_bytes(proto_len).map_err(|_| ClientHelloError::MalformedExtension)?;
		if let Ok(s) = std::str::from_utf8(proto_bytes) {
			info.alpn.push(s.to_owned());
		}
	}
	Ok(())
}

fn parse_supported_versions(
	c: &mut Cursor<'_>,
	ext_end: usize,
	info: &mut ClientHelloInfo,
) -> Result<(), ClientHelloError> {
	if c.pos >= ext_end {
		return Err(ClientHelloError::MalformedExtension);
	}
	let list_len = c.read_u8().map_err(|_| ClientHelloError::MalformedExtension)? as usize;
	if !list_len.is_multiple_of(2) || c.pos + list_len > ext_end {
		return Err(ClientHelloError::MalformedExtension);
	}
	for _ in 0..list_len / 2 {
		let ver = c.read_u16().map_err(|_| ClientHelloError::MalformedExtension)?;
		info.supported_versions.push(ver);
	}
	Ok(())
}

fn parse_supported_groups(
	c: &mut Cursor<'_>,
	ext_end: usize,
	info: &mut ClientHelloInfo,
) -> Result<(), ClientHelloError> {
	if c.pos + 2 > ext_end {
		return Err(ClientHelloError::MalformedExtension);
	}
	let list_len = c.read_u16().map_err(|_| ClientHelloError::MalformedExtension)? as usize;
	if !list_len.is_multiple_of(2) || c.pos + list_len > ext_end {
		return Err(ClientHelloError::MalformedExtension);
	}
	for _ in 0..list_len / 2 {
		let group = c.read_u16().map_err(|_| ClientHelloError::MalformedExtension)?;
		info.supported_groups.push(group);
	}
	Ok(())
}

// -- Cursor: lightweight position-tracking reader --

struct Cursor<'a> {
	data: &'a [u8],
	pos: usize,
}

impl<'a> Cursor<'a> {
	const fn new(data: &'a [u8]) -> Self {
		Self { data, pos: 0 }
	}

	const fn remaining(&self) -> usize {
		self.data.len().saturating_sub(self.pos)
	}

	fn read_u8(&mut self) -> Result<u8, ClientHelloError> {
		if self.pos >= self.data.len() {
			return Err(ClientHelloError::TooShort);
		}
		let v = self.data[self.pos];
		self.pos += 1;
		Ok(v)
	}

	fn read_u16(&mut self) -> Result<u16, ClientHelloError> {
		if self.pos + 2 > self.data.len() {
			return Err(ClientHelloError::TooShort);
		}
		let v = u16::from_be_bytes([self.data[self.pos], self.data[self.pos + 1]]);
		self.pos += 2;
		Ok(v)
	}

	fn read_u24(&mut self) -> Result<u32, ClientHelloError> {
		if self.pos + 3 > self.data.len() {
			return Err(ClientHelloError::TooShort);
		}
		let v = u32::from(self.data[self.pos]) << 16
			| u32::from(self.data[self.pos + 1]) << 8
			| u32::from(self.data[self.pos + 2]);
		self.pos += 3;
		Ok(v)
	}

	fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], ClientHelloError> {
		if self.pos + n > self.data.len() {
			return Err(ClientHelloError::TooShort);
		}
		let slice = &self.data[self.pos..self.pos + n];
		self.pos += n;
		Ok(slice)
	}

	const fn skip(&mut self, n: usize) -> Result<(), ClientHelloError> {
		if self.pos + n > self.data.len() {
			return Err(ClientHelloError::TooShort);
		}
		self.pos += n;
		Ok(())
	}
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
	use super::*;
	use std::sync::Arc;
	use tokio::net::TcpListener;

	/// Capture raw `ClientHello` bytes by starting a TLS handshake and peeking on
	/// the server side before rustls processes the data.
	async fn capture_client_hello(alpn: &[&str]) -> Vec<u8> {
		let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
		let addr = listener.local_addr().unwrap();

		let alpn_owned: Vec<Vec<u8>> = alpn.iter().map(|s| s.as_bytes().to_vec()).collect();

		let client_handle = tokio::spawn(async move {
			let provider = Arc::new(rustls::crypto::ring::default_provider());
			let mut config = rustls::ClientConfig::builder_with_provider(provider)
				.with_safe_default_protocol_versions()
				.unwrap()
				.dangerous()
				.with_custom_certificate_verifier(Arc::new(NoVerify))
				.with_no_client_auth();
			config.alpn_protocols = alpn_owned;

			let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
			let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
			let server_name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
			// The connect will fail since server doesn't respond, but
			// that's fine — we only need the ClientHello bytes sent.
			let _ = connector.connect(server_name, tcp).await;
		});

		let (stream, _) = listener.accept().await.unwrap();

		// Peek up to 4096 bytes to capture the full ClientHello
		let raw = crate::tcp::peek_tcp(&stream, 4096).await.unwrap();
		let captured = raw.to_vec();

		// Shut down cleanly
		stream.into_std().unwrap().shutdown(std::net::Shutdown::Both).ok();
		let _ = client_handle.await;

		captured
	}

	#[derive(Debug)]
	struct NoVerify;

	impl rustls::client::danger::ServerCertVerifier for NoVerify {
		fn verify_server_cert(
			&self,
			_: &rustls::pki_types::CertificateDer<'_>,
			_: &[rustls::pki_types::CertificateDer<'_>],
			_: &rustls::pki_types::ServerName<'_>,
			_: &[u8],
			_: rustls::pki_types::UnixTime,
		) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
			Ok(rustls::client::danger::ServerCertVerified::assertion())
		}
		fn verify_tls12_signature(
			&self,
			_: &[u8],
			_: &rustls::pki_types::CertificateDer<'_>,
			_: &rustls::DigitallySignedStruct,
		) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
			Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
		}
		fn verify_tls13_signature(
			&self,
			_: &[u8],
			_: &rustls::pki_types::CertificateDer<'_>,
			_: &rustls::DigitallySignedStruct,
		) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
			Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
		}
		fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
			rustls::crypto::ring::default_provider().signature_verification_algorithms.supported_schemes()
		}
	}

	#[tokio::test]
	async fn real_clienthello_parses() {
		let raw = capture_client_hello(&[]).await;
		let info = parse_client_hello(&raw).unwrap();

		assert_eq!(info.sni.as_deref(), Some("localhost"));
		assert!(!info.cipher_suites.is_empty());
		assert!(!info.supported_versions.is_empty());
	}

	#[tokio::test]
	async fn real_clienthello_with_alpn() {
		let raw = capture_client_hello(&["h2", "http/1.1"]).await;
		let info = parse_client_hello(&raw).unwrap();

		assert_eq!(info.sni.as_deref(), Some("localhost"));
		assert!(info.alpn.contains(&"h2".to_owned()));
		assert!(info.alpn.contains(&"http/1.1".to_owned()));
	}

	#[test]
	fn truncated_before_extensions() {
		// Minimal ClientHello: record header + handshake header + version + random
		// + empty session_id + 2 cipher suites + 1 compression method
		// Then slice right after compression — no extensions
		let mut data = Vec::new();
		// Record header
		data.push(0x16); // handshake
		data.extend_from_slice(&[0x03, 0x03]); // TLS 1.2
		// Record length placeholder (filled below)
		let record_len_pos = data.len();
		data.extend_from_slice(&[0x00, 0x00]);

		let body_start = data.len();

		// Handshake header
		data.push(0x01); // ClientHello
		let hs_len_pos = data.len();
		data.extend_from_slice(&[0x00, 0x00, 0x00]); // length placeholder

		let hs_body_start = data.len();

		// ClientHello body
		data.extend_from_slice(&[0x03, 0x03]); // version
		data.extend_from_slice(&[0u8; 32]); // random
		data.push(0x00); // session_id length = 0
		data.extend_from_slice(&[0x00, 0x04]); // cipher suites length = 4
		data.extend_from_slice(&[0x13, 0x01]); // TLS_AES_128_GCM_SHA256
		data.extend_from_slice(&[0x13, 0x02]); // TLS_AES_256_GCM_SHA384
		data.push(0x01); // compression methods length = 1
		data.push(0x00); // null compression

		// Fill lengths
		let hs_len = data.len() - hs_body_start;
		data[hs_len_pos] = (hs_len >> 16) as u8;
		data[hs_len_pos + 1] = (hs_len >> 8) as u8;
		data[hs_len_pos + 2] = hs_len as u8;

		let record_len = data.len() - body_start;
		data[record_len_pos] = (record_len >> 8) as u8;
		data[record_len_pos + 1] = record_len as u8;

		let info = parse_client_hello(&data).unwrap();
		assert!(info.sni.is_none());
		assert!(info.alpn.is_empty());
		assert_eq!(info.cipher_suites, vec![0x1301, 0x1302]);
	}

	#[tokio::test]
	async fn truncated_mid_extension() {
		let raw = capture_client_hello(&[]).await;
		// Slice at roughly 60% of the data to cut inside extensions
		let cut = raw.len() * 60 / 100;
		let truncated = &raw[..cut];
		let result = parse_client_hello(truncated);
		// Should succeed with partial results (SNI may or may not be present
		// depending on where the cut falls)
		assert!(result.is_ok());
	}

	#[test]
	fn non_tls_data() {
		let data = b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n";
		assert_eq!(parse_client_hello(data), Err(ClientHelloError::NotHandshake));
	}

	#[test]
	fn not_clienthello() {
		// Valid handshake record header but handshake type 0x02 (ServerHello)
		let data = [0x16, 0x03, 0x03, 0x00, 0x05, 0x02, 0x00, 0x00, 0x01, 0x00];
		assert_eq!(parse_client_hello(&data), Err(ClientHelloError::NotClientHello));
	}

	#[test]
	fn empty_data() {
		assert_eq!(parse_client_hello(&[]), Err(ClientHelloError::TooShort));
	}

	#[test]
	fn sanitize_sni_cases() {
		assert_eq!(sanitize_sni("Example.COM"), "example.com");
		assert_eq!(sanitize_sni("foo@bar!baz"), "foobarbaz");
		assert_eq!(sanitize_sni("my-host.example_test.com"), "my-host.example_test.com");
		assert_eq!(sanitize_sni(""), "");
	}

	#[test]
	fn minimal_clienthello_no_extensions() {
		let data = build_minimal_clienthello(None);
		let info = parse_client_hello(&data).unwrap();
		assert!(info.sni.is_none());
		assert!(info.alpn.is_empty());
		assert!(info.supported_versions.is_empty());
		assert_eq!(info.cipher_suites, vec![0x1301]);
	}

	#[test]
	fn odd_cipher_suite_length() {
		let mut data = Vec::new();
		// Record header
		data.push(0x16);
		data.extend_from_slice(&[0x03, 0x03]);
		// Placeholder for record length
		let record_len_pos = data.len();
		data.extend_from_slice(&[0x00, 0x00]);
		let body_start = data.len();

		// Handshake header
		data.push(0x01);
		let hs_len_pos = data.len();
		data.extend_from_slice(&[0x00, 0x00, 0x00]);
		let hs_body_start = data.len();

		// ClientHello body
		data.extend_from_slice(&[0x03, 0x03]); // version
		data.extend_from_slice(&[0u8; 32]); // random
		data.push(0x00); // session_id length = 0
		data.extend_from_slice(&[0x00, 0x03]); // cipher suites length = 3 (odd!)
		data.extend_from_slice(&[0x13, 0x01, 0x00]); // 3 bytes

		// Fill lengths
		let hs_len = data.len() - hs_body_start;
		data[hs_len_pos] = (hs_len >> 16) as u8;
		data[hs_len_pos + 1] = (hs_len >> 8) as u8;
		data[hs_len_pos + 2] = hs_len as u8;
		let record_len = data.len() - body_start;
		data[record_len_pos] = (record_len >> 8) as u8;
		data[record_len_pos + 1] = record_len as u8;

		assert_eq!(parse_client_hello(&data), Err(ClientHelloError::InvalidLength));
	}

	#[test]
	fn non_utf8_sni() {
		// Build SNI extension with non-UTF8 bytes
		let mut ext = Vec::new();
		ext.extend_from_slice(&[0x00, 0x00]); // extension type: SNI
		let name_bytes: &[u8] = &[0xFF, 0xFE];
		let sni_list_len = 3 + name_bytes.len();
		let ext_data_len = 2 + sni_list_len;
		ext.extend_from_slice(&(ext_data_len as u16).to_be_bytes());
		ext.extend_from_slice(&(sni_list_len as u16).to_be_bytes());
		ext.push(0x00); // host_name type
		ext.extend_from_slice(&(name_bytes.len() as u16).to_be_bytes());
		ext.extend_from_slice(name_bytes);

		let data = build_clienthello_with_raw_extensions(&ext);
		let info = parse_client_hello(&data).unwrap();
		assert!(info.sni.is_none());
	}

	#[test]
	fn clienthello_with_sni_handcrafted() {
		let data = build_minimal_clienthello(Some("example.com"));
		let info = parse_client_hello(&data).unwrap();
		assert_eq!(info.sni.as_deref(), Some("example.com"));
		assert_eq!(info.cipher_suites, vec![0x1301]);
	}

	#[test]
	fn extension_length_overflow() {
		// Craft an extension with length 0xFFFF but only 10 bytes of data
		let mut ext = Vec::new();
		ext.extend_from_slice(&[0x00, 0x42]); // unknown extension type
		ext.extend_from_slice(&[0xFF, 0xFF]); // extension length = 65535 (overflow)
		ext.extend_from_slice(&[0x00; 10]); // only 10 bytes of actual data

		let data = build_clienthello_with_raw_extensions(&ext);
		let info = parse_client_hello(&data).unwrap();
		// Parser breaks gracefully — returns Ok with partial results
		assert!(info.sni.is_none());
	}

	#[test]
	fn supported_versions_odd_length() {
		// supported_versions extension (type 0x002b)
		let mut ext = Vec::new();
		ext.extend_from_slice(&[0x00, 0x2b]); // extension type: supported_versions
		ext.extend_from_slice(&[0x00, 0x04]); // extension length = 4
		ext.push(0x03); // list_len = 3 (odd!)
		ext.extend_from_slice(&[0x03, 0x04, 0x03]); // 3 bytes of version data

		let data = build_clienthello_with_raw_extensions(&ext);
		assert_eq!(parse_client_hello(&data), Err(ClientHelloError::MalformedExtension));
	}

	/// Build a `ClientHello` with raw extensions bytes injected directly.
	fn build_clienthello_with_raw_extensions(extensions: &[u8]) -> Vec<u8> {
		let mut body = Vec::new();
		body.extend_from_slice(&[0x03, 0x03]); // version TLS 1.2
		body.extend_from_slice(&[0u8; 32]); // random
		body.push(0x00); // session_id length = 0
		body.extend_from_slice(&[0x00, 0x02]); // cipher suites length = 2
		body.extend_from_slice(&[0x13, 0x01]); // TLS_AES_128_GCM_SHA256
		body.push(0x01); // compression methods length = 1
		body.push(0x00); // null compression

		// Extensions length + raw data
		body.extend_from_slice(&(extensions.len() as u16).to_be_bytes());
		body.extend_from_slice(extensions);

		let mut data = Vec::new();
		data.push(0x16); // handshake
		data.extend_from_slice(&[0x03, 0x01]); // TLS 1.0 (record version)
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

	/// Build a minimal hand-crafted `ClientHello` with optional SNI.
	fn build_minimal_clienthello(sni: Option<&str>) -> Vec<u8> {
		let mut body = Vec::new();

		// ClientHello body
		body.extend_from_slice(&[0x03, 0x03]); // version TLS 1.2
		body.extend_from_slice(&[0u8; 32]); // random
		body.push(0x00); // session_id length = 0
		body.extend_from_slice(&[0x00, 0x02]); // cipher suites length = 2
		body.extend_from_slice(&[0x13, 0x01]); // TLS_AES_128_GCM_SHA256
		body.push(0x01); // compression methods length = 1
		body.push(0x00); // null compression

		if let Some(hostname) = sni {
			let name_bytes = hostname.as_bytes();
			// SNI extension
			let mut ext = Vec::new();
			ext.extend_from_slice(&[0x00, 0x00]); // extension type: SNI
			let sni_list_len = 3 + name_bytes.len(); // name_type(1) + name_len(2) + name
			let ext_data_len = 2 + sni_list_len; // list_len(2) + sni_list
			ext.extend_from_slice(&(ext_data_len as u16).to_be_bytes());
			ext.extend_from_slice(&(sni_list_len as u16).to_be_bytes());
			ext.push(0x00); // host_name type
			ext.extend_from_slice(&(name_bytes.len() as u16).to_be_bytes());
			ext.extend_from_slice(name_bytes);

			// Extensions length
			body.extend_from_slice(&(ext.len() as u16).to_be_bytes());
			body.extend(ext);
		}

		let mut data = Vec::new();
		// Record header
		data.push(0x16); // handshake
		data.extend_from_slice(&[0x03, 0x01]); // TLS 1.0 (record version)
		let record_len = 4 + body.len(); // handshake header (4) + body
		data.extend_from_slice(&(record_len as u16).to_be_bytes());

		// Handshake header
		data.push(0x01); // ClientHello
		let hs_len = body.len();
		data.push((hs_len >> 16) as u8);
		data.push((hs_len >> 8) as u8);
		data.push(hs_len as u8);

		data.extend(body);
		data
	}
}
