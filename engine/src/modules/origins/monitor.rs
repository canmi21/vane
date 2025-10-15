/* engine/src/modules/origins/monitor.rs */

use crate::{
	common::response,
	modules::origins::{
		state::{
			MONITOR_CONFIG_FILE_LOCK, MONITOR_REPORTS, MonitorConfig, NEXT_CHECK_TIME, TASK_STATUS,
			TRIGGER_CHANNEL, TaskStatus,
		},
		task::{load_monitor_config, save_monitor_config},
	},
};
use axum::{
	Json,
	extract::Path,
	http::StatusCode,
	response::{IntoResponse, Response},
};
use fancy_log::{LogLevel, log};
use serde::Deserialize;

// --- API Payloads ---

#[derive(Deserialize)]
pub struct UpdatePeriodPayload {
	pub period_seconds: u64,
}

#[derive(Deserialize)]
pub struct OverridePayload {
	pub origin_id: String,
	pub url: String,
}

// --- Axum Handlers ---

/// Returns the current status of all monitored origins.
pub async fn get_monitor_status() -> impl IntoResponse {
	log(LogLevel::Debug, "GET /v1/monitor/origins called");
	let reports = MONITOR_REPORTS.read().await;
	response::success(reports.clone())
}

/// Returns the current status of the background monitoring task.
pub async fn get_task_status() -> impl IntoResponse {
	log(
		LogLevel::Debug,
		"GET /v1/monitor/origins/task-status called",
	);
	let status = TASK_STATUS.read().await;
	response::success(status.clone())
}

/// Returns the timestamp of the next scheduled check.
pub async fn get_next_check_time() -> impl IntoResponse {
	log(LogLevel::Debug, "GET /v1/monitor/origins/next-check called");
	let next_time = NEXT_CHECK_TIME.read().await;
	response::success(*next_time)
}

/// Manually triggers an origin check cycle.
pub async fn trigger_check_now() -> Response {
	log(
		LogLevel::Info,
		"POST /v1/monitor/origins/trigger-check called",
	);
	{
		let status = TASK_STATUS.read().await;
		if *status == TaskStatus::Running {
			return response::error(
				StatusCode::CONFLICT,
				"A check is already in progress.".to_string(),
			)
			.into_response();
		}
	} // Release the read lock

	if let Err(e) = TRIGGER_CHANNEL.send(()) {
		log(
			LogLevel::Error,
			&format!("Failed to send trigger signal: {}", e),
		);
	}

	(StatusCode::ACCEPTED, "Check triggered successfully.").into_response()
}

/// Updates the check interval for the origin monitor.
pub async fn update_check_period(Json(payload): Json<UpdatePeriodPayload>) -> Response {
	log(LogLevel::Info, "PUT /v1/monitor/origins/period called");
	if payload.period_seconds < 10 {
		return response::error(
			StatusCode::BAD_REQUEST,
			"Period must be at least 10 seconds.".to_string(),
		)
		.into_response();
	}

	let _lock = MONITOR_CONFIG_FILE_LOCK.lock().await;
	let mut config = load_monitor_config().await;
	config.period_seconds = payload.period_seconds;

	if let Err(e) = save_monitor_config(&config).await {
		log(
			LogLevel::Error,
			&format!("Failed to save monitor config: {}", e),
		);
		return response::error(
			StatusCode::INTERNAL_SERVER_ERROR,
			"Failed to save monitor configuration.".to_string(),
		)
		.into_response();
	}

	log(
		LogLevel::Info,
		&format!(
			"Monitor check period updated to {}s",
			payload.period_seconds
		),
	);
	response::success(config).into_response()
}

/// Creates or updates an override URL for a specific origin.
pub async fn set_override_url(Json(payload): Json<OverridePayload>) -> Response {
	log(LogLevel::Info, "PUT /v1/monitor/origins/override called");

	let _lock = MONITOR_CONFIG_FILE_LOCK.lock().await;
	let mut config = load_monitor_config().await;

	config
		.overrides
		.insert(payload.origin_id.clone(), payload.url.clone());

	if let Err(e) = save_monitor_config(&config).await {
		log(
			LogLevel::Error,
			&format!("Failed to save monitor config: {}", e),
		);
		return response::error(
			StatusCode::INTERNAL_SERVER_ERROR,
			"Failed to save monitor configuration.".to_string(),
		)
		.into_response();
	}

	log(
		LogLevel::Info,
		&format!("Override URL set for origin ID: {}", payload.origin_id),
	);
	response::success(config).into_response()
}

/// Deletes an override URL for a specific origin.
pub async fn delete_override_url(Path(origin_id): Path<String>) -> Response {
	log(
		LogLevel::Info,
		&format!("DELETE /v1/monitor/origins/override/{} called", origin_id),
	);

	let _lock = MONITOR_CONFIG_FILE_LOCK.lock().await;
	let mut config: MonitorConfig = load_monitor_config().await;

	if config.overrides.remove(&origin_id).is_none() {
		return response::error(
			StatusCode::NOT_FOUND,
			"Override for the given origin ID not found.".to_string(),
		)
		.into_response();
	}

	if let Err(e) = save_monitor_config(&config).await {
		log(
			LogLevel::Error,
			&format!("Failed to save monitor config: {}", e),
		);
		return response::error(
			StatusCode::INTERNAL_SERVER_ERROR,
			"Failed to save monitor configuration.".to_string(),
		)
		.into_response();
	}

	log(
		LogLevel::Info,
		&format!("Override URL removed for origin ID: {}", origin_id),
	);
	StatusCode::NO_CONTENT.into_response()
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::modules::origins::state::{TASK_STATUS, TRIGGER_CHANNEL, TaskStatus};
	use axum::Json;
	use serial_test::serial;
	use std::env;
	use tempfile::tempdir;
	use tokio::time::timeout;

	async fn setup_temp_config_env() -> tempfile::TempDir {
		let tmp_dir = tempdir().unwrap();
		let config_path = tmp_dir.path().join("origin_monitor.json");
		tokio::fs::write(&config_path, "{}").await.unwrap();
		unsafe {
			env::set_var("CONFIG_PATH", tmp_dir.path().to_str().unwrap());
		};
		tmp_dir
	}

	#[tokio::test]
	#[serial]
	async fn test_update_check_period_too_short() {
		let _tmp_dir_guard = setup_temp_config_env().await;
		let payload = UpdatePeriodPayload { period_seconds: 5 };
		let response = update_check_period(Json(payload)).await;
		assert_eq!(response.into_response().status(), StatusCode::BAD_REQUEST);
	}

	#[tokio::test]
	#[serial]
	async fn test_trigger_check_now_conflict() {
		*TASK_STATUS.write().await = TaskStatus::Running;
		let response = trigger_check_now().await;
		assert_eq!(response.status(), StatusCode::CONFLICT);
		*TASK_STATUS.write().await = TaskStatus::Idle; // Reset state
	}

	#[tokio::test]
	#[serial]
	async fn test_trigger_check_now_idle() {
		// Subscribe to the channel to ensure it's not "closed" for this test.
		let mut receiver = TRIGGER_CHANNEL.subscribe();

		*TASK_STATUS.write().await = TaskStatus::Idle;
		let response = trigger_check_now().await;
		assert_eq!(response.status(), StatusCode::ACCEPTED);

		// Assert that the trigger signal was actually received.
		let recv_result = timeout(std::time::Duration::from_secs(1), receiver.recv()).await;
		assert!(recv_result.is_ok(), "Trigger signal was not received");
	}
}
