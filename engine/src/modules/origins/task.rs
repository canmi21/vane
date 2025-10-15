/* engine/src/modules/origins/task.rs */

use crate::{
	daemon::config,
	modules::origins::{
		origins::Origin,
		state::{
			MONITOR_REPORTS, MonitorConfig, NEXT_CHECK_TIME, OriginMonitorReport, OriginStatus,
			TASK_STATUS, TRIGGER_CHANNEL, TaskStatus,
		},
	},
};
use chrono::{Duration as ChronoDuration, Utc};
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

/// Checks if `origin_monitor.json` exists, and creates it with default values if not.
pub async fn initialize_monitor_config() {
	let path = config::get_monitor_config_path();
	if tokio::fs::metadata(&path).await.is_err() {
		log(
			LogLevel::Info,
			&format!("Creating default monitor config at {}", path.display()),
		);
		let default_config = MonitorConfig::default();
		if let Err(e) = save_monitor_config(&default_config).await {
			log(
				LogLevel::Error,
				&format!("Failed to create default monitor config file: {}", e),
			);
		}
	}
}

/// Spawns the background task that periodically checks origin health.
pub fn start_monitoring_task() {
	log(
		LogLevel::Info,
		"Starting origin monitoring background task.",
	);
	tokio::spawn(async move {
		let clients = HttpClients {
			default: Client::builder()
				.timeout(Duration::from_secs(10))
				.build()
				.expect("Failed to build default reqwest client"),
			insecure: Client::builder()
				.timeout(Duration::from_secs(10))
				.danger_accept_invalid_certs(true)
				.build()
				.expect("Failed to build insecure reqwest client"),
		};

		let mut trigger_receiver = TRIGGER_CHANNEL.subscribe();

		// Perform an initial check immediately on startup.
		log(
			LogLevel::Debug,
			"Performing initial origin check on startup.",
		);
		perform_check_and_update_state(&clients).await;

		loop {
			let monitor_config = load_monitor_config().await;
			let check_interval = monitor_config.period_seconds;

			// Update the next scheduled check time for the API
			let next_time = Utc::now() + ChronoDuration::seconds(check_interval as i64);
			*NEXT_CHECK_TIME.write().await = Some(next_time);

			log(
				LogLevel::Debug,
				&format!(
					"Origin monitor is idle. Next check in {} seconds or on manual trigger.",
					check_interval
				),
			);

			// Wait for either the timer to expire or a manual trigger.
			tokio::select! {
				_ = sleep(Duration::from_secs(check_interval)) => {
					log(LogLevel::Debug, "Scheduled check triggered by timer.");
				}
				result = trigger_receiver.recv() => {
					if result.is_ok() {
						log(LogLevel::Info, "Check triggered manually via API.");
					} else {
						// This can happen if the channel lags, it's safe to just continue.
						log(LogLevel::Warn, "Trigger channel error, likely due to lag. Continuing with next cycle.");
						continue; // Skip this cycle and restart the loop to get a fresh timer.
					}
				}
			}

			// After waking up, perform the check cycle.
			perform_check_and_update_state(&clients).await;
		}
	});
}

/// A wrapper function that sets the task state, runs the check, and resets the state.
async fn perform_check_and_update_state(clients: &HttpClients) {
	// Set state to Running
	*TASK_STATUS.write().await = TaskStatus::Running;
	// Clear the next check time as we are running now.
	*NEXT_CHECK_TIME.write().await = None;

	let monitor_config = load_monitor_config().await;
	run_check_cycle(clients, &monitor_config).await;

	// Set state back to Idle
	*TASK_STATUS.write().await = TaskStatus::Idle;
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

	let mut join_handles = Vec::new();
	for (id, origin) in origins {
		let default_client = clients.default.clone();
		let insecure_client = clients.insecure.clone();
		let override_url = monitor_config.overrides.get(&id).cloned();
		join_handles.push(tokio::spawn(async move {
			check_single_origin(default_client, insecure_client, id, origin, override_url).await
		}));
	}

	let results = future::join_all(join_handles).await;

	let mut reports_writer = MONITOR_REPORTS.write().await;
	reports_writer.clear();
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

	let client_to_use = if origin.scheme == "https" && origin.skip_ssl_verify {
		&insecure_client
	} else {
		&default_client
	};

	let request_builder = client_to_use.get(&check_url);

	let (status, message) = match request_builder.send().await {
		Ok(response) => {
			let http_status = response.status();
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

#[cfg(test)]
mod tests {
	use super::*;
	use crate::modules::origins::state::MONITOR_REPORTS;
	use serial_test::serial;
	use std::env;
	use tempfile::tempdir;

	async fn setup_temp_config_env() -> tempfile::TempDir {
		let tmp_dir = tempdir().unwrap();
		let cfg_path = tmp_dir.path().join("origin_monitor.json");
		tokio::fs::write(&cfg_path, "{}").await.unwrap();
		unsafe {
			env::set_var("CONFIG_PATH", tmp_dir.path().to_str().unwrap());
		};
		tmp_dir
	}

	#[tokio::test]
	#[serial]
	async fn test_run_check_cycle_empty_file() {
		// This sets up a temp directory with origin_monitor.json and sets CONFIG_PATH
		let tmp_dir_guard = setup_temp_config_env().await;
		let config_dir = tmp_dir_guard.path();

		// Create the origins.json file in the SAME temporary directory
		let origins_path = config_dir.join("origins.json");
		tokio::fs::write(&origins_path, "{}").await.unwrap();

		let clients = HttpClients {
			default: Client::builder().build().unwrap(),
			insecure: Client::builder()
				.danger_accept_invalid_certs(true)
				.build()
				.unwrap(),
		};

		// The test target function will now find both config files in the correct path
		let cfg = load_monitor_config().await;
		run_check_cycle(&clients, &cfg).await;

		assert!(MONITOR_REPORTS.read().await.is_empty());
	}

	#[tokio::test]
	#[serial]
	async fn test_check_single_origin_timeout() {
		let clients = HttpClients {
			default: Client::builder()
				.timeout(Duration::from_millis(10))
				.build()
				.unwrap(),
			insecure: Client::builder()
				.timeout(Duration::from_millis(10))
				.danger_accept_invalid_certs(true)
				.build()
				.unwrap(),
		};
		let origin = Origin {
			scheme: "http".into(),
			host: "10.255.255.1".into(), // Use a non-routable IP to guarantee a timeout
			port: 80,
			path: "/".into(),
			skip_ssl_verify: false,
			raw_url: "http://10.255.255.1".into(),
		};
		let (_, report) = check_single_origin(
			clients.default.clone(),
			clients.insecure.clone(),
			"timeout-test".into(),
			origin,
			None,
		)
		.await;

		assert_eq!(report.status, OriginStatus::Unhealthy);
		assert!(
			report
				.last_message
				.unwrap_or_default()
				.contains("timed out"),
			"Error message should indicate a timeout"
		);
	}
}
