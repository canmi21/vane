/* engine/src/modules/origins/monitor.rs */

use crate::{
	common::response,
	modules::origins::{
		state::{MONITOR_CONFIG_FILE_LOCK, MONITOR_REPORTS, MonitorConfig},
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
