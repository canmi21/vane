/* src/modules/stack/transport/loader.rs */

use super::model::{TcpConfig, UdpConfig};
use fancy_log::{LogLevel, log};
use serde::de::DeserializeOwned;
use std::{fs, path::Path};
use validator::Validate;

const EXTENSIONS: [&str; 3] = ["toml", "yaml", "json"];

/// A trait to abstract the pre-processing of loaded configs before validation.
pub trait PreProcess {
	fn pre_process(&mut self);
}

impl PreProcess for TcpConfig {
	fn pre_process(&mut self) {
		for rule in &mut self.rules {
			rule.name = rule.name.to_lowercase();
		}
	}
}

impl PreProcess for UdpConfig {
	fn pre_process(&mut self) {
		for rule in &mut self.rules {
			rule.name = rule.name.to_lowercase();
		}
	}
}

/// Loads, parses, and validates a config file for a given protocol and port.
pub fn load_config<T>(port: u16, protocol_name: &str, base_path: &Path) -> Option<T>
where
	T: DeserializeOwned + Validate + PreProcess,
{
	let mut found_files = Vec::new();
	for ext in EXTENSIONS {
		let path = base_path.with_extension(ext);
		if path.exists() {
			found_files.push(path);
		}
	}

	if found_files.len() > 1 {
		log(
			LogLevel::Warn,
			&format!(
				"✗ Found multiple config files for {} on port {}: {:?}. Deactivating.",
				protocol_name.to_uppercase(),
				port,
				found_files
			),
		);
		return None;
	}

	let config_path = found_files.first()?;
	let content = match fs::read_to_string(config_path) {
		Ok(c) => c,
		Err(e) => {
			log(
				LogLevel::Error,
				&format!(
					"✗ Failed to read config file {}: {}",
					config_path.display(),
					e
				),
			);
			return None;
		}
	};

	let ext = config_path
		.extension()
		.and_then(|s| s.to_str())
		.unwrap_or("");

	let config_result: Result<T, String> = match ext {
		"toml" => toml::from_str(&content).map_err(|e| e.to_string()),
		"yaml" => serde_yaml::from_str(&content).map_err(|e| e.to_string()),
		"json" => serde_json::from_str(&content).map_err(|e| e.to_string()),
		_ => return None,
	};

	match config_result {
		Ok(mut config) => {
			config.pre_process();
			match config.validate() {
				Ok(_) => Some(config),
				Err(e) => {
					log(
						LogLevel::Error,
						&format!("✗ Validation failed for {}: {}", config_path.display(), e),
					);
					None
				}
			}
		}
		Err(e) => {
			log(
				LogLevel::Error,
				&format!(
					"✗ Failed to parse config file {}: {}",
					config_path.display(),
					e
				),
			);
			None
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use serial_test::serial;
	use tempfile::tempdir;

	// --- Helper Constants for Test Data ---

	const VALID_CONFIG_TOML: &str = r#"
[[protocols]]
name = "http"
priority = 1
detect = { method = "prefix", pattern = "GET" }
destination = { type = "forward", forward = { strategy = "random", targets = [{ip = "127.0.0.1", port = 80}] } }
"#;

	const VALID_CONFIG_YAML: &str = r#"
protocols:
  - name: http
    priority: 1
    detect:
      method: prefix
      pattern: "GET"
    destination:
      type: forward
      forward:
        strategy: random
        targets:
          - ip: 127.0.0.1
            port: 80
"#;

	const VALID_CONFIG_JSON: &str = r#"
{
  "protocols": [
    {
      "name": "http",
      "priority": 1,
      "detect": { "method": "prefix", "pattern": "GET" },
      "destination": {
        "type": "forward",
        "forward": {
          "strategy": "random",
          "targets": [{ "ip": "127.0.0.1", "port": 80 }]
        }
      }
    }
  ]
}
"#;

	const CONFIG_WITH_UPPERCASE_NAME: &str = r#"
[[protocols]]
name = "MyHttp"
priority = 1
detect = { method = "prefix", pattern = "GET" }
destination = { type = "forward", forward = { strategy = "random", targets = [{ip = "127.0.0.1", port = 80}] } }
"#;

	const INVALID_SYNTAX_TOML: &str = "this is not valid toml";

	const INVALID_DATA_CONFIG: &str = r#"
[[protocols]]
name = "http"
priority = 0 # Priority must be >= 1, this will fail validation.
detect = { method = "prefix", pattern = "GET" }
destination = { type = "forward", forward = { strategy = "random", targets = [{ip = "127.0.0.1", port = 80}] } }
"#;

	/// Tests that the loader can successfully parse all supported file formats.
	#[test]
	#[serial]
	fn test_load_config_success_all_formats() {
		let temp_dir = tempdir().unwrap();
		let base_path = temp_dir.path().join("tcp");

		// Test TOML
		fs::write(base_path.with_extension("toml"), VALID_CONFIG_TOML).unwrap();
		let config = load_config::<TcpConfig>(80, "tcp", &base_path);
		assert!(config.is_some(), "Should successfully load TOML");
		fs::remove_file(base_path.with_extension("toml")).unwrap();

		// Test YAML
		fs::write(base_path.with_extension("yaml"), VALID_CONFIG_YAML).unwrap();
		let config = load_config::<TcpConfig>(80, "tcp", &base_path);
		assert!(config.is_some(), "Should successfully load YAML");
		fs::remove_file(base_path.with_extension("yaml")).unwrap();

		// Test JSON
		fs::write(base_path.with_extension("json"), VALID_CONFIG_JSON).unwrap();
		let config = load_config::<TcpConfig>(80, "tcp", &base_path);
		assert!(config.is_some(), "Should successfully load JSON");
	}

	/// Tests that the loader returns None when multiple conflicting config files exist.
	#[test]
	#[serial]
	fn test_load_config_conflict() {
		let temp_dir = tempdir().unwrap();
		let base_path = temp_dir.path().join("tcp");

		fs::write(base_path.with_extension("toml"), VALID_CONFIG_TOML).unwrap();
		fs::write(base_path.with_extension("json"), VALID_CONFIG_JSON).unwrap();

		let config = load_config::<TcpConfig>(80, "tcp", &base_path);
		assert!(
			config.is_none(),
			"Should return None when conflicting files exist"
		);
	}

	/// Tests that the loader returns None when the config file has a syntax error.
	#[test]
	#[serial]
	fn test_load_config_parsing_error() {
		let temp_dir = tempdir().unwrap();
		let base_path = temp_dir.path().join("tcp");
		fs::write(base_path.with_extension("toml"), INVALID_SYNTAX_TOML).unwrap();

		let config = load_config::<TcpConfig>(80, "tcp", &base_path);
		assert!(
			config.is_none(),
			"Should return None for a file with invalid syntax"
		);
	}

	/// Tests that the loader returns None when the config data fails validation.
	#[test]
	#[serial]
	fn test_load_config_validation_error() {
		let temp_dir = tempdir().unwrap();
		let base_path = temp_dir.path().join("tcp");
		fs::write(base_path.with_extension("toml"), INVALID_DATA_CONFIG).unwrap();

		let config = load_config::<TcpConfig>(80, "tcp", &base_path);
		assert!(
			config.is_none(),
			"Should return None for a config that fails validation"
		);
	}

	/// Tests that the pre-processing step is correctly applied to a loaded config.
	#[test]
	#[serial]
	fn test_pre_processing_logic() {
		let temp_dir = tempdir().unwrap();
		let base_path = temp_dir.path().join("tcp");
		fs::write(base_path.with_extension("toml"), CONFIG_WITH_UPPERCASE_NAME).unwrap();

		let config = load_config::<TcpConfig>(80, "tcp", &base_path);
		assert!(config.is_some());

		let processed_config = config.unwrap();
		assert_eq!(
			processed_config.rules[0].name, "myhttp",
			"Rule name should be lowercased by pre_process"
		);
	}
}
