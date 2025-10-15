/* engine/src/modules/certs/analysis.rs */

use oid_registry::*;
use serde::Serialize;
use sha1::{Digest, Sha1};
use sha2::Sha256;
use x509_parser::{pem::Pem, prelude::*};

// --- Data Structures for Certificate Details ---

#[derive(Serialize)]
pub struct CertInfo {
	pub subject: NameInfo,
	pub issuer: NameInfo,
	pub validity: ValidityInfo,
	pub subject_alternative_names: Vec<String>,
	pub public_key: PublicKeyInfo,
	pub serial_number: String,
	pub signature: SignatureInfo,
	pub fingerprints: Fingerprints,
	pub is_ca: bool,
}

#[derive(Serialize)]
pub struct NameInfo {
	pub common_name: Option<String>,
	pub organization: Option<String>,
	pub organizational_unit: Option<String>,
	pub country: Option<String>,
	pub state: Option<String>,
	pub locality: Option<String>,
	pub raw_string: String,
}

#[derive(Serialize)]
pub struct ValidityInfo {
	pub not_before: String,
	pub not_after: String,
	pub is_valid: bool,
}

#[derive(Serialize)]
pub struct PublicKeyInfo {
	pub algorithm: String,
	pub key_size_bits: usize,
}

#[derive(Serialize)]
pub struct SignatureInfo {
	pub algorithm: String,
	pub value: String,
}

#[derive(Serialize)]
pub struct Fingerprints {
	pub sha1: String,
	pub sha256: String,
}

/// Parses the raw bytes of a certificate into a detailed `CertInfo` struct.
pub fn parse_cert_details(cert_bytes: &[u8]) -> Result<CertInfo, String> {
	// Try to parse as PEM first, fallback to DER
	let raw_data = match Pem::read(std::io::Cursor::new(cert_bytes)) {
		Ok((pem, _)) => pem.contents,
		Err(_) => cert_bytes.to_vec(),
	};

	let (_, cert) = X509Certificate::from_der(&raw_data)
		.map_err(|e| format!("Failed to parse X.509 certificate: {}", e))?;

	Ok(CertInfo {
		subject: name_to_info(cert.subject()),
		issuer: name_to_info(cert.issuer()),
		validity: validity_to_info(cert.validity()),
		subject_alternative_names: get_sans(&cert).unwrap_or_default(),
		public_key: public_key_to_info(cert.public_key()),
		serial_number: format_serial(&cert.serial),
		signature: signature_to_info(&cert.signature_algorithm, &cert.signature_value),
		fingerprints: calculate_fingerprints(&raw_data),
		is_ca: cert.is_ca(),
	})
}

// --- Helper Functions ---

/// Helper to get the first attribute value for a given OID from an X509Name.
fn get_first_attr<'a>(name: &'a X509Name, oid: &Oid) -> Option<String> {
	name
		.iter_by_oid(oid)
		.next()
		.and_then(|attr| attr.as_str().ok().map(String::from))
}

/// Converts an X509Name into a more structured `NameInfo`.
fn name_to_info(name: &X509Name) -> NameInfo {
	NameInfo {
		common_name: get_first_attr(name, &OID_X509_COMMON_NAME),
		organization: get_first_attr(name, &OID_X509_ORGANIZATION_NAME),
		organizational_unit: get_first_attr(name, &OID_X509_ORGANIZATIONAL_UNIT),
		country: get_first_attr(name, &OID_X509_COUNTRY_NAME),
		state: get_first_attr(name, &OID_X509_STATE_OR_PROVINCE_NAME),
		locality: get_first_attr(name, &OID_X509_LOCALITY_NAME),
		raw_string: name.to_string(),
	}
}

/// Converts a `Validity` struct into `ValidityInfo`.
fn validity_to_info(validity: &Validity) -> ValidityInfo {
	let now = ASN1Time::now();
	let not_before = validity.not_before;
	let not_after = validity.not_after;

	ValidityInfo {
		is_valid: now >= not_before && now < not_after,
		not_before: not_before
			.to_rfc2822()
			.unwrap_or_else(|_| not_before.to_string()),
		not_after: not_after
			.to_rfc2822()
			.unwrap_or_else(|_| not_after.to_string()),
	}
}

/// Extracts Subject Alternative Names (SANs) from certificate extensions.
fn get_sans(cert: &X509Certificate) -> Result<Vec<String>, X509Error> {
	cert
		.subject_alternative_name()?
		.map(|ext| {
			ext
				.value
				.general_names
				.iter()
				.map(|gn| gn.to_string())
				.collect()
		})
		.ok_or(X509Error::InvalidExtensions)
}

/// Extracts public key algorithm and size.
fn public_key_to_info(pk: &SubjectPublicKeyInfo) -> PublicKeyInfo {
	PublicKeyInfo {
		algorithm: pk.algorithm.algorithm.to_id_string(),
		key_size_bits: pk.subject_public_key.data.len() * 8,
	}
}

/// Formats the serial number into a hex string.
fn format_serial(serial: &num_bigint::BigUint) -> String {
	format!("{:x}", serial)
}

/// Extracts signature algorithm and value.
fn signature_to_info<'a>(
	alg: &AlgorithmIdentifier,
	value: &asn1_rs::BitString<'a>,
) -> SignatureInfo {
	SignatureInfo {
		algorithm: alg.algorithm.to_id_string(),
		value: hex::encode(value.data.as_ref()),
	}
}

/// Calculates SHA1 and SHA256 fingerprints of the certificate.
fn calculate_fingerprints(data: &[u8]) -> Fingerprints {
	let mut sha1_hasher = Sha1::new();
	sha1_hasher.update(data);
	let sha1_result = sha1_hasher.finalize();

	let mut sha256_hasher = Sha256::new();
	sha256_hasher.update(data);
	let sha256_result = sha256_hasher.finalize();

	Fingerprints {
		sha1: hex::encode(sha1_result),
		sha256: hex::encode(sha256_result),
	}
}
