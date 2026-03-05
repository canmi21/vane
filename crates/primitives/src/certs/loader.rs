/* src/resources/certs/loader.rs */

use crate::common::{
	config::file_loader,
	sys::{hotswap::watch_loop, lifecycle::Result},
};
use crate::certs::{arcswap, format};
use fancy_log::{LogLevel, log};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::fs;
use tokio::sync::mpsc;
use x509_parser::prelude::{FromDer, parse_x509_pem};

struct CertCandidate {
	crt: Option<PathBuf>,
	pem: Option<PathBuf>,
	key: Option<PathBuf>,
}
impl CertCandidate {
	fn new() -> Self {
		Self {
			crt: None,
			pem: None,
			key: None,
		}
	}
}

async fn ensure_default_certificate() {
	let config_dir = file_loader::get_config_dir().join("certs");
	if fs::metadata(&config_dir).await.is_err() {
		let _ = fs::create_dir_all(&config_dir).await;
	}
	let cert_path = config_dir.join("default.crt");
	let key_path = config_dir.join("default.key");
	let mut should_generate = false;
	if fs::metadata(&cert_path).await.is_err() || fs::metadata(&key_path).await.is_err() {
		should_generate = true;
	} else if let Ok(expiring) = check_cert_expiration(&cert_path).await
		&& expiring
	{
		should_generate = true;
	}
	if should_generate {
		log(LogLevel::Info, "⚙ Generating default certificate...");
		let _ = generate_self_signed(&cert_path, &key_path).await;
	}
}

async fn check_cert_expiration(cert_path: &Path) -> Result<bool> {
	let content = fs::read(cert_path)
		.await
		.map_err(|e| crate::common::sys::lifecycle::Error::Io(e.to_string()))?;
	let (_, pem) = parse_x509_pem(&content)
		.map_err(|e| crate::common::sys::lifecycle::Error::Tls(format!("PEM error: {e}")))?;
	let (_, x509) = x509_parser::certificate::X509Certificate::from_der(&pem.contents)
		.map_err(|e| crate::common::sys::lifecycle::Error::Tls(format!("X509 error: {e}")))?;
	let not_after = x509.validity.not_after.timestamp();
	let now = SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.unwrap_or_default()
		.as_secs() as i64;
	Ok(not_after - now < 7 * 24 * 60 * 60)
}

async fn generate_self_signed(cert_path: &Path, key_path: &Path) -> Result<()> {
	let san = vec!["localhost".to_owned(), "127.0.0.1".to_owned()];
	let ck = rcgen::generate_simple_self_signed(san)
		.map_err(|e| crate::common::sys::lifecycle::Error::Tls(format!("rcgen error: {e}")))?;
	let _ = fs::write(cert_path, ck.cert.pem()).await;
	let _ = fs::write(key_path, ck.signing_key.serialize_pem()).await;
	Ok(())
}

pub async fn scan_and_load_certs() {
	let config_dir = file_loader::get_config_dir().join("certs");
	if fs::metadata(&config_dir).await.is_err() {
		return;
	}
	let snapshot = arcswap::CERT_REGISTRY.snapshot();
	let mut new_state: HashMap<String, Arc<arcswap::LoadedCert>> = snapshot
		.iter()
		.map(|(k, v)| (k.clone(), Arc::clone(&v.value)))
		.collect();
	let Ok(mut entries) = fs::read_dir(&config_dir).await else {
		return;
	};
	let mut candidates: HashMap<String, CertCandidate> = HashMap::new();
	while let Ok(Some(entry)) = entries.next_entry().await {
		let path = entry.path();
		if !path.is_file() {
			continue;
		}
		if let Some(filename) = path.file_name().and_then(|s| s.to_str())
			&& let Some(dot_idx) = filename.rfind('.')
		{
			let stem = filename[..dot_idx].to_string();
			let ext = &filename[dot_idx + 1..];
			let record = candidates.entry(stem).or_insert_with(CertCandidate::new);
			match ext {
				"crt" => record.crt = Some(path),
				"pem" => record.pem = Some(path),
				"key" => record.key = Some(path),
				_ => {}
			}
		}
	}
	for (id, candidate) in candidates {
		let Some(key_path) = candidate.key else {
			continue;
		};
		let cert_path = candidate.crt.or(candidate.pem);
		if let Some(c_path) = cert_path
			&& let Ok(ck) = format::load_and_validate_pair(&c_path, &key_path).await
		{
			new_state.insert(id, ck);
		}
	}
	arcswap::update_registry(new_state);
}

pub async fn listen_for_updates(rx: mpsc::Receiver<()>) {
	watch_loop(rx, "Certificates", || async {
		scan_and_load_certs().await;
	})
	.await;
}

pub async fn initialize() {
	ensure_default_certificate().await;
	scan_and_load_certs().await;
}
