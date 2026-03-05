/* src/api/handlers/certs.rs */

use crate::response;
use crate::schemas::certs::{
	CertDetail, CertDetailResponse, CertListResponse, CertOperationResponse, CertOperationResult,
	CertSummary, CertUploadRequest,
};
use axum::{Json, extract::Path, http::StatusCode, response::IntoResponse};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::fs;
use vane_primitives::certs::arcswap;
use vane_primitives::common::config::file_loader;
use x509_parser::prelude::*;

// --- Helpers ---

fn parse_cert_summary(id: &str, der: &[u8]) -> Option<CertSummary> {
	let (_, x509) = X509Certificate::from_der(der).ok()?;
	let subject = x509.subject().to_string();
	let issuer = x509.issuer().to_string();
	let not_before = x509.validity().not_before.to_string();
	let not_after = x509.validity().not_after.to_string();

	let mut hasher = Sha256::new();
	hasher.update(der);
	let fingerprint = hex::encode_upper(hasher.finalize());

	Some(CertSummary {
		id: id.to_owned(),
		subject,
		issuer,
		not_before,
		not_after,
		fingerprint_sha256: fingerprint,
		auto_generated: id == "default",
	})
}

fn parse_cert_detail(id: &str, der: &[u8]) -> Option<CertDetail> {
	let (_, x509) = X509Certificate::from_der(der).ok()?;
	let subject = x509.subject().to_string();
	let issuer = x509.issuer().to_string();
	let not_before = x509.validity().not_before.to_string();
	let not_after_dt = x509.validity().not_after;
	let not_after = not_after_dt.to_string();

	let now = SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.unwrap_or_default()
		.as_secs() as i64;
	let expires_in_days = (not_after_dt.timestamp() - now) / 86400;

	let mut hasher = Sha256::new();
	hasher.update(der);
	let fingerprint = hex::encode_upper(hasher.finalize());

	let mut san = Vec::new();
	if let Ok(Some(ext)) = x509.subject_alternative_name() {
		for name in &ext.value.general_names {
			san.push(format!("{name:?}"));
		}
	}

	let key_type = format!("{:?}", x509.public_key().algorithm.algorithm);

	Some(CertDetail {
		id: id.to_owned(),
		subject,
		issuer,
		not_before,
		not_after,
		expires_in_days,
		fingerprint_sha256: fingerprint,
		san,
		key_type,
	})
}

// --- Handlers ---

/// List all certificates
#[utoipa::path(
    get,
    path = "/certs",
    responses(
        (status = 200, description = "List of certificates", body = CertListResponse)
    ),
    tag = "certs",
    security(("bearer_auth" = []))
)]
pub async fn list_certs_handler() -> impl IntoResponse {
	let snapshot = arcswap::CERT_REGISTRY.snapshot();
	let mut certs = Vec::new();

	for (id, entry) in snapshot.iter() {
		if let Some(first_der) = entry.value.certs.first()
			&& let Some(summary) = parse_cert_summary(id, first_der)
		{
			certs.push(summary);
		}
	}

	certs.sort_by_key(|c| c.id.clone());
	response::success(certs)
}

/// Get certificate details
#[utoipa::path(
    get,
    path = "/certs/{id}",
    params(
        ("id" = String, Path, description = "Certificate ID")
    ),
    responses(
        (status = 200, description = "Certificate details", body = CertDetailResponse),
        (status = 404, description = "Certificate not found")
    ),
    tag = "certs",
    security(("bearer_auth" = []))
)]
pub async fn get_cert_handler(Path(id): Path<String>) -> impl IntoResponse {
	if let Some(loaded) = arcswap::CERT_REGISTRY.get(&id)
		&& let Some(first_der) = loaded.certs.first()
		&& let Some(detail) = parse_cert_detail(&id, first_der)
	{
		return response::success(detail);
	}

	response::error(
		StatusCode::NOT_FOUND,
		format!("Certificate '{id}' not found"),
	)
}

/// Upload certificate
#[utoipa::path(
    post,
    path = "/certs/{id}",
    params(
        ("id" = String, Path, description = "Certificate ID")
    ),
    request_body = CertUploadRequest,
    responses(
        (status = 201, description = "Certificate uploaded", body = CertOperationResponse),
        (status = 400, description = "Invalid certificate or key")
    ),
    tag = "certs",
    security(("bearer_auth" = []))
)]
pub async fn upload_cert_handler(
	Path(id): Path<String>,
	Json(req): Json<CertUploadRequest>,
) -> impl IntoResponse {
	// 1. Basic validation of PEM
	if !req.cert_pem.contains("BEGIN CERTIFICATE") || !req.key_pem.contains("PRIVATE KEY") {
		return response::error(StatusCode::BAD_REQUEST, "Invalid PEM format".into());
	}

	// 2. Save to disk
	let certs_dir = file_loader::get_config_dir().join("certs");
	if fs::metadata(&certs_dir).await.is_err() {
		let _ = fs::create_dir_all(&certs_dir).await;
	}

	let cert_path = certs_dir.join(format!("{id}.crt"));
	let key_path = certs_dir.join(format!("{id}.key"));

	if let Err(e) = fs::write(&cert_path, &req.cert_pem).await {
		return response::error(
			StatusCode::INTERNAL_SERVER_ERROR,
			format!("Failed to write cert: {e}"),
		);
	}
	if let Err(e) = fs::write(&key_path, &req.key_pem).await {
		return response::error(
			StatusCode::INTERNAL_SERVER_ERROR,
			format!("Failed to write key: {e}"),
		);
	}

	// 3. Try to parse metadata for response
	let mut subject = None;
	let mut not_after = None;

	// Simple extraction for the response
	if let Some(start) = req.cert_pem.find("-----BEGIN CERTIFICATE-----") {
		let cert_part = &req.cert_pem[start..];
		if let Some(end) = cert_part.find("-----END CERTIFICATE-----") {
			let pem_bytes = &cert_part.as_bytes()[..end + 25];
			if let Ok((_, pem)) = x509_parser::pem::parse_x509_pem(pem_bytes)
				&& let Ok((_, x509)) = X509Certificate::from_der(&pem.contents)
			{
				subject = Some(x509.subject().to_string());
				not_after = Some(x509.validity().not_after.to_string());
			}
		}
	}

	response::created(CertOperationResult {
		id,
		created: true,
		subject,
		not_after,
	})
}

/// Delete certificate
#[utoipa::path(
    delete,
    path = "/certs/{id}",
    params(
        ("id" = String, Path, description = "Certificate ID")
    ),
    responses(
        (status = 200, description = "Certificate deleted", body = CertOperationResponse),
        (status = 400, description = "Cannot delete default certificate"),
        (status = 404, description = "Certificate not found")
    ),
    tag = "certs",
    security(("bearer_auth" = []))
)]
pub async fn delete_cert_handler(Path(id): Path<String>) -> impl IntoResponse {
	if id == "default" {
		return response::error(
			StatusCode::BAD_REQUEST,
			"Cannot delete default certificate".into(),
		);
	}

	let certs_dir = file_loader::get_config_dir().join("certs");
	let cert_path = certs_dir.join(format!("{id}.crt"));
	let key_path = certs_dir.join(format!("{id}.key"));
	let pem_path = certs_dir.join(format!("{id}.pem"));

	let mut found = false;
	if fs::metadata(&cert_path).await.is_ok() {
		let _ = fs::remove_file(&cert_path).await;
		found = true;
	}
	if fs::metadata(&pem_path).await.is_ok() {
		let _ = fs::remove_file(&pem_path).await;
		found = true;
	}
	if fs::metadata(&key_path).await.is_ok() {
		let _ = fs::remove_file(&key_path).await;
		found = true;
	}

	if !found {
		return response::error(
			StatusCode::NOT_FOUND,
			format!("Certificate '{id}' not found"),
		);
	}

	response::success(CertOperationResult {
		id,
		created: false,
		subject: None,
		not_after: None,
	})
}
