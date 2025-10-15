/* engine/src/modules/origins/state.rs */

use chrono::{DateTime, Utc};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{Mutex, RwLock};

// --- Monitor Configuration (for origin_monitor.json) ---

// --- 1. REMOVE `Default` FROM THE DERIVE MACRO ---
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

// --- 2. MANUALLY IMPLEMENT THE DEFAULT TRAIT ---
// This ensures that when the config file doesn't exist, we use our desired defaults.
impl Default for MonitorConfig {
	fn default() -> Self {
		Self {
			period_seconds: default_period(), // Correctly use the 300s default
			overrides: HashMap::new(),
		}
	}
}

// --- Shared State for Monitor Reports ---

#[derive(Serialize, Clone, Debug)]
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
