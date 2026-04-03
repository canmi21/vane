use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Declarative configuration for a single engine instance.
///
/// ```
/// use std::collections::HashMap;
/// use vane_engine::config::{ConfigTable, GlobalConfig, ListenConfig, PortConfig, TargetAddr};
///
/// let config = ConfigTable {
///     ports: HashMap::from([(
///         8080,
///         PortConfig {
///             listen: ListenConfig::default(),
///             target: TargetAddr { ip: "127.0.0.1".to_owned(), port: 3000 },
///         },
///     )]),
///     global: GlobalConfig::default(),
/// };
///
/// let json = serde_json::to_string(&config).unwrap();
/// let back: ConfigTable = serde_json::from_str(&json).unwrap();
/// assert_eq!(config, back);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ConfigTable {
	pub ports: HashMap<u16, PortConfig>,
	#[serde(default)]
	pub global: GlobalConfig,
}

/// Per-port configuration: listen settings and a forward target.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortConfig {
	#[serde(default)]
	pub listen: ListenConfig,
	pub target: TargetAddr,
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

/// Listen configuration for a port.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ListenConfig {
	#[serde(default)]
	pub ipv6: bool,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
	use super::*;

	fn sample_config() -> ConfigTable {
		ConfigTable {
			ports: HashMap::from([(
				8080,
				PortConfig {
					listen: ListenConfig::default(),
					target: TargetAddr { ip: "127.0.0.1".to_owned(), port: 3000 },
				},
			)]),
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
	fn listen_config_defaults() {
		let json = "{}";
		let listen: ListenConfig = serde_json::from_str(json).unwrap();
		assert!(!listen.ipv6);
	}
}
