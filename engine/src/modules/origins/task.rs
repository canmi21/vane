/* engine/src/modules/origins/task.rs */

use crate::{
	daemon::config,
	modules::origins::{
		origins::Origin,
		state::{MONITOR_REPORTS, MonitorConfig, OriginMonitorReport, OriginStatus},
	},
};
use chrono::Utc;
use fancy_log::{LogLevel, log};
use futures::future;
use reqwest::Client;
use std::{collections::HashMap, time::Duration};
use tokio::time::sleep;

// A struct to hold the different reqwest clients we'll need.
struct HttpClients {
	default: Client,
	insecure: Client,
}

// --- NEW FUNCTION TO INITIALIZE THE CONFIG FILE ---
/// Checks if `origin_monitor.json` exists, and creates it with default values if not.
pub async fn initialize_monitor_config() {
	let path = config::get_monitor_config_path();
	// Use `metadata` to check for file existence asynchronously.
	if tokio::fs::metadata(&path).await.is_err() {
		log(
			LogLevel::Info,
			&format!("Creating default monitor config at {}", path.display()),
		);
		// This now correctly uses the manual `impl Default` we created.
		let default_config = MonitorConfig::default();
		if let Err(e) = save_monitor_config(&default_config).await {
			log(
				LogLevel::Error,
				&format!("Failed to create default monitor config file: {}", e),
			);
		}
	}
}
// ---------------------------------------------------

/// Spawns the background task that periodically checks origin health.
pub fn start_monitoring_task() {
	log(
		LogLevel::Info,
		"Starting origin monitoring background task.",
	);
	tokio::spawn(async move {
		// Create two clients upfront: one for standard requests and one for insecure (skipping SSL verification) requests.
		let clients = HttpClients {
			// Default client with a 10-second timeout.
			default: Client::builder()
				.timeout(Duration::from_secs(10))
				.build()
				.expect("Failed to build default reqwest client"),
			// Client that skips SSL certificate verification.
			insecure: Client::builder()
				.timeout(Duration::from_secs(10))
				.danger_accept_invalid_certs(true)
				.build()
				.expect("Failed to build insecure reqwest client"),
		};

		loop {
			// Load monitor configuration first to get the interval
			let monitor_config = load_monitor_config().await;
			let check_interval = monitor_config.period_seconds;

			log(
				LogLevel::Debug,
				&format!(
					"Origin monitor is running a check cycle. Next check in {} seconds.",
					check_interval
				),
			);

			run_check_cycle(&clients, &monitor_config).await;

			// Wait for the configured interval before the next cycle
			sleep(Duration::from_secs(check_interval)).await;
		}
	});
}

/// Performs a single cycle of checking all origins.
async fn run_check_cycle(clients: &HttpClients, monitor_config: &MonitorConfig) {
	let origins_path = config::get_origins_path();
	if !origins_path.exists() {
		log(
			LogLevel::Debug,
			"origins.json not found, skipping check cycle.",
		);
		return;
	}

	let content = match tokio::fs::read_to_string(origins_path).await {
		Ok(c) => c,
		Err(e) => {
			log(
				LogLevel::Error,
				&format!("Failed to read origins.json: {}", e),
			);
			return;
		}
	};

	let origins: HashMap<String, Origin> = match serde_json::from_str(&content) {
		Ok(o) => o,
		Err(e) => {
			log(
				LogLevel::Error,
				&format!("Failed to parse origins.json: {}", e),
			);
			return;
		}
	};

	// If there are no origins, clear the reports and do nothing.
	if origins.is_empty() {
		let mut reports = MONITOR_REPORTS.write().await;
		if !reports.is_empty() {
			log(
				LogLevel::Info,
				"No origins found, clearing monitor reports.",
			);
			reports.clear();
		}
		return;
	}

	// Concurrently check all origins
	let mut join_handles = Vec::new();
	for (id, origin) in origins {
		// Clone references to the clients for the spawned task
		let default_client = clients.default.clone();
		let insecure_client = clients.insecure.clone();
		let override_url = monitor_config.overrides.get(&id).cloned();
		join_handles.push(tokio::spawn(async move {
			check_single_origin(default_client, insecure_client, id, origin, override_url).await
		}));
	}

	let results = future::join_all(join_handles).await;

	// Update the shared state with the new reports
	let mut reports_writer = MONITOR_REPORTS.write().await;
	reports_writer.clear(); // Clear old reports for origins that may have been deleted
	for result in results {
		if let Ok((id, report)) = result {
			reports_writer.insert(id, report);
		}
	}
	log(LogLevel::Debug, "Finished origin check cycle.");
}

/// Checks a single origin and returns its report.
async fn check_single_origin(
	default_client: Client,
	insecure_client: Client,
	id: String,
	origin: Origin,
	override_url: Option<String>,
) -> (String, OriginMonitorReport) {
	let check_url = override_url.unwrap_or_else(|| {
		let path = if origin.path.starts_with('/') {
			&origin.path
		} else {
			"/"
		};
		format!(
			"{}://{}:{}{}",
			origin.scheme, origin.host, origin.port, path
		)
	});

	// Choose the correct client based on the origin's SSL verification setting.
	let client_to_use = if origin.scheme == "https" && origin.skip_ssl_verify {
		&insecure_client
	} else {
		&default_client
	};

	let request_builder = client_to_use.get(&check_url);

	let (status, message) = match request_builder.send().await {
		Ok(response) => {
			let http_status = response.status();
			// is_success() checks for any 2xx status code.
			if http_status.is_success() {
				(OriginStatus::Healthy, format!("OK ({})", http_status))
			} else {
				(
					OriginStatus::Unhealthy,
					format!("Received non-2xx status: {}", http_status),
				)
			}
		}
		Err(e) => {
			let error_msg = if e.is_timeout() {
				"Request timed out after 10 seconds.".to_string()
			} else {
				e.to_string()
			};
			(OriginStatus::Unhealthy, error_msg)
		}
	};

	let report = OriginMonitorReport {
		status,
		check_url,
		last_checked: Some(Utc::now()),
		last_message: Some(message),
	};

	(id, report)
}

/// Loads the monitor configuration from `origin_monitor.json`.
pub async fn load_monitor_config() -> MonitorConfig {
	let path = config::get_monitor_config_path();
	match tokio::fs::read_to_string(path).await {
		Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
		Err(_) => MonitorConfig::default(),
	}
}

/// Saves the monitor configuration to `origin_monitor.json`.
pub async fn save_monitor_config(config: &MonitorConfig) -> Result<(), std::io::Error> {
	let path = config::get_monitor_config_path();
	let contents = serde_json::to_string_pretty(config).unwrap();
	tokio::fs::write(path, contents).await
}
