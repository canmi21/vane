/* src/core/src/lazycert/sync.rs */

use super::LAZYCERT_CLIENT;
use super::client::LazyCertClient;
use anyhow::Result;
use fancy_log::{LogLevel, log};
use once_cell::sync::Lazy;
use sigterm::CancellationToken;
use std::sync::Arc;
use std::time::Duration;
use tokio::fs;
use tokio::sync::RwLock;
use vane_primitives::certs::loader::scan_and_load_certs;
use vane_primitives::common::config::file_loader;
use vane_primitives::lazycert::{CHALLENGE_REGISTRY, ChallengeEntry};

/// Global cancellation token for sync task
static SYNC_CANCEL: Lazy<RwLock<Option<CancellationToken>>> = Lazy::new(|| RwLock::new(None));

/// Spawn background task for LazyCert synchronization
pub fn spawn_sync_task(client: Arc<LazyCertClient>, poll_interval: Duration) {
	tokio::spawn(async move {
		// Cancel any existing sync task
		{
			let mut cancel_lock = SYNC_CANCEL.write().await;
			if let Some(old_token) = cancel_lock.take() {
				old_token.cancel();
			}

			// Create new cancellation channel
			let token = CancellationToken::new();
			*cancel_lock = Some(token.clone());

			tokio::spawn(async move {
				let mut interval = tokio::time::interval(poll_interval);

				loop {
					tokio::select! {
					_ = interval.tick() => {
							// Check if client is still valid (hot-reload may have changed it)
							let current_client = if let Some(lock) = LAZYCERT_CLIENT.get() {
									lock.read().await.clone()
							} else {
									None
							};

							// Stop if client was removed or changed
							if current_client.is_none() || !Arc::ptr_eq(&client, current_client.as_ref().unwrap()) {
									log(LogLevel::Debug, "LazyCert client changed, stopping old sync task");
									break;
							}

																	// Sync challenges
																	if let Err(e) = sync_challenges(&client).await {
																					log(
																									LogLevel::Warn,
																									&format!("Failed to sync challenges: {e}"),
																					);
																	}
													}
													_ = token.cancelled() => {
																	log(LogLevel::Debug, "LazyCert sync task cancelled");
																	break;
													}
									}
				}
			});
		}
	});
}

/// Sync pending challenges from LazyCert
async fn sync_challenges(client: &LazyCertClient) -> Result<()> {
	let challenges = client.get_challenges().await?;

	for ch in challenges {
		// Only handle HTTP-01 challenges
		if ch.r#type != "http-01" {
			continue;
		}

		// Check if already registered
		if CHALLENGE_REGISTRY.contains_key(&ch.token) {
			continue;
		}

		// Register challenge
		log(
			LogLevel::Info,
			&format!("Registering HTTP-01 challenge for domain: {}", ch.domain),
		);

		CHALLENGE_REGISTRY.insert(
			ch.token.clone(),
			ChallengeEntry::new(
				ch.key_authorization.clone(),
				ch.domain.clone(),
				ch.id.clone(),
			),
		);

		// Notify LazyCert that challenge is ready
		if let Err(e) = client.mark_challenge_solved(&ch.id).await {
			log(
				LogLevel::Error,
				&format!("Failed to mark challenge solved: {e}"),
			);
			// Remove from registry since LazyCert doesn't know we're ready
			CHALLENGE_REGISTRY.remove(&ch.token);
		} else {
			log(
				LogLevel::Info,
				&format!("Challenge ready for domain: {}", ch.domain),
			);
		}
	}

	Ok(())
}

/// Download certificate from response and save to certs/
pub async fn save_certificate_from_response(
	cert_id: &str,
	cert_pem: &str,
	key_pem: &str,
) -> Result<()> {
	let certs_dir = file_loader::get_config_dir().join("certs");

	// Ensure directory exists
	if fs::metadata(&certs_dir).await.is_err() {
		fs::create_dir_all(&certs_dir).await?;
	}

	let cert_path = certs_dir.join(format!("{cert_id}.crt"));
	let key_path = certs_dir.join(format!("{cert_id}.key"));

	// Atomic write via temp files
	let cert_temp = certs_dir.join(format!("{cert_id}.crt.tmp"));
	let key_temp = certs_dir.join(format!("{cert_id}.key.tmp"));

	fs::write(&cert_temp, cert_pem).await?;
	fs::write(&key_temp, key_pem).await?;

	fs::rename(&cert_temp, &cert_path).await?;
	fs::rename(&key_temp, &key_path).await?;

	log(
		LogLevel::Info,
		&format!("Saved certificate '{cert_id}' to certs/"),
	);

	// Trigger hot-reload
	scan_and_load_certs().await;

	Ok(())
}
