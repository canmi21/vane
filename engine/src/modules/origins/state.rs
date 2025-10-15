/* engine/src/modules/origins/state.rs */

use chrono::{DateTime, Utc};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{Mutex, RwLock, broadcast};

// --- Monitor Configuration (for origin_monitor.json) ---

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct MonitorConfig {
	#[serde(default = "default_period")]
	pub period_seconds: u64,
	#[serde(default)]
	pub overrides: HashMap<String, String>,
}

fn default_period() -> u64 {
	300 // 5 minutes
}

impl Default for MonitorConfig {
	fn default() -> Self {
		Self {
			period_seconds: default_period(),
			overrides: HashMap::new(),
		}
	}
}

// --- Shared State for Monitor Reports ---

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum OriginStatus {
	Healthy,
	Unhealthy,
	Pending,
}

#[derive(Serialize, Clone, Debug)]
pub struct OriginMonitorReport {
	pub status: OriginStatus,
	pub check_url: String,
	pub last_checked: Option<DateTime<Utc>>,
	pub last_message: Option<String>,
}

pub type MonitorReportsStore = HashMap<String, OriginMonitorReport>;

/// Holds the latest health check report for each origin.
/// This is the shared state updated by the background task and read by the API.
pub static MONITOR_REPORTS: Lazy<Arc<RwLock<MonitorReportsStore>>> =
	Lazy::new(|| Arc::new(RwLock::new(HashMap::new())));

/// A mutex to ensure safe, sequential writes to the `origin_monitor.json` file.
pub static MONITOR_CONFIG_FILE_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

// --- Task State Machine and Trigger ---

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
	Idle,
	Running,
}

/// Holds the current status of the monitoring task.
pub static TASK_STATUS: Lazy<Arc<RwLock<TaskStatus>>> =
	Lazy::new(|| Arc::new(RwLock::new(TaskStatus::Idle)));

/// Holds the timestamp for the next scheduled check.
pub static NEXT_CHECK_TIME: Lazy<Arc<RwLock<Option<DateTime<Utc>>>>> =
	Lazy::new(|| Arc::new(RwLock::new(None)));

/// A channel to manually trigger a check cycle.
pub static TRIGGER_CHANNEL: Lazy<broadcast::Sender<()>> = Lazy::new(|| broadcast::channel(1).0);

// --- Tests ---
#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_default_monitor_config() {
		let cfg = MonitorConfig::default();
		assert_eq!(cfg.period_seconds, 300);
		assert!(cfg.overrides.is_empty());
	}

	#[test]
	fn test_origin_status_serialization() {
		let s = serde_json::to_string(&OriginStatus::Healthy).unwrap();
		assert!(s.contains("healthy"));
	}

	#[tokio::test]
	async fn test_shared_state_defaults() {
		assert!(MONITOR_REPORTS.read().await.is_empty());
		assert_eq!(*TASK_STATUS.read().await, TaskStatus::Idle);
		assert!(NEXT_CHECK_TIME.read().await.is_none());
	}

	#[tokio::test]
	async fn test_monitor_config_lock_is_mutex() {
		let _lock1 = MONITOR_CONFIG_FILE_LOCK.lock().await;
		assert!(true);
	}
}
