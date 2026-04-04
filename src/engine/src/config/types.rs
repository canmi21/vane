use serde::{Deserialize, Serialize};

use super::listener::ListenerRule;

/// Declarative configuration for a single engine instance.
///
/// ```
/// use vane_engine::config::{ConfigTable, GlobalConfig, ListenerRule, Protocol, TargetAddr};
///
/// let config = ConfigTable {
///     listeners: vec![ListenerRule {
///         bind: "0.0.0.0".to_owned(),
///         port: "8080".to_owned(),
///         protocol: Protocol::Tcp,
///     }],
///     target: Some(TargetAddr { ip: "127.0.0.1".to_owned(), port: 3000 }),
///     global: GlobalConfig::default(),
/// };
///
/// let json = serde_json::to_string(&config).unwrap();
/// let back: ConfigTable = serde_json::from_str(&json).unwrap();
/// assert_eq!(config, back);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ConfigTable {
	#[serde(default)]
	pub listeners: Vec<ListenerRule>,
	#[serde(default)]
	pub target: Option<TargetAddr>,
	#[serde(default)]
	pub global: GlobalConfig,
}

/// TCP forward target address.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TargetAddr {
	pub ip: String,
	pub port: u16,
}

/// Global engine settings with sensible defaults.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GlobalConfig {
	#[serde(default = "default_max_connections")]
	pub max_connections: usize,
	#[serde(default = "default_max_connections_per_ip")]
	pub max_connections_per_ip: usize,
}

const fn default_max_connections() -> usize {
	10_000
}
const fn default_max_connections_per_ip() -> usize {
	50
}

impl Default for GlobalConfig {
	fn default() -> Self {
		Self {
			max_connections: default_max_connections(),
			max_connections_per_ip: default_max_connections_per_ip(),
		}
	}
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
	use super::*;
	use crate::config::Protocol;

	fn sample_config() -> ConfigTable {
		ConfigTable {
			listeners: vec![ListenerRule {
				bind: "0.0.0.0".to_owned(),
				port: "8080".to_owned(),
				protocol: Protocol::Tcp,
			}],
			target: Some(TargetAddr { ip: "127.0.0.1".to_owned(), port: 3000 }),
			global: GlobalConfig::default(),
		}
	}

	#[test]
	fn json_serde_roundtrip() {
		let config = sample_config();
		let json = serde_json::to_string_pretty(&config).unwrap();
		let back: ConfigTable = serde_json::from_str(&json).unwrap();
		assert_eq!(config, back);
	}

	#[test]
	fn global_config_defaults() {
		let json = "{}";
		let global: GlobalConfig = serde_json::from_str(json).unwrap();
		assert_eq!(global, GlobalConfig::default());
	}

	#[test]
	fn empty_config_defaults() {
		let json = "{}";
		let config: ConfigTable = serde_json::from_str(json).unwrap();
		assert!(config.listeners.is_empty());
		assert!(config.target.is_none());
	}
}
