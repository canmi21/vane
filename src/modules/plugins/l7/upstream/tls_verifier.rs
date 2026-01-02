/* src/modules/plugins/l7/upstream/tls_verifier.rs */

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::{
	DigitallySignedStruct,
	pki_types::{CertificateDer, ServerName, UnixTime},
};

#[derive(Debug)]
pub struct NoVerifier;

impl ServerCertVerifier for NoVerifier {
	fn verify_server_cert(
		&self,
		_end_entity: &CertificateDer<'_>,
		_intermediates: &[CertificateDer<'_>],
		_server_name: &ServerName<'_>,
		_ocsp_response: &[u8],
		_now: UnixTime,
	) -> Result<ServerCertVerified, rustls::Error> {
		Ok(ServerCertVerified::assertion())
	}

	fn verify_tls12_signature(
		&self,
		_message: &[u8],
		_cert: &CertificateDer<'_>,
		_dss: &DigitallySignedStruct,
	) -> Result<HandshakeSignatureValid, rustls::Error> {
		Ok(HandshakeSignatureValid::assertion())
	}

	fn verify_tls13_signature(
		&self,
		_message: &[u8],
		_cert: &CertificateDer<'_>,
		_dss: &DigitallySignedStruct,
	) -> Result<HandshakeSignatureValid, rustls::Error> {
		Ok(HandshakeSignatureValid::assertion())
	}

	fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
		vec![
			rustls::SignatureScheme::RSA_PKCS1_SHA1,
			rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
			rustls::SignatureScheme::RSA_PSS_SHA256,
			rustls::SignatureScheme::ED25519,
		]
	}
}
