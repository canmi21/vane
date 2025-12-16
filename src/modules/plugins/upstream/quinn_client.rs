/* src/modules/plugins/upstream/quinn_client.rs */

use crate::common::requirements::{Error, Result};
use fancy_log::{LogLevel, log};

pub async fn execute_quinn_request(url_str: &str, skip_verify: bool) -> Result<()> {
	// 1. Validation Logic
	if !url_str.starts_with("https://") {
		log(LogLevel::Error, "✗ QUIC/H3 requires HTTPS scheme.");
		return Err(Error::Configuration(
			"QUIC upstream must use https://".into(),
		));
	}

	// 2. TODO Log
	log(
		LogLevel::Debug,
		"⚙ TODO: H3 Upstream Client (Quinn) is not yet implemented.",
	);
	log(
		LogLevel::Debug,
		&format!("  Target: {} (Skip Verify: {})", url_str, skip_verify),
	);

	// 3. Return Error to prevent silent failure
	Err(Error::System(
		"H3 Upstream Engine not implemented in v0.5.4".into(),
	))
}
