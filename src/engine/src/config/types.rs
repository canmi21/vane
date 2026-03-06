use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Declarative configuration for a single engine instance.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfigTable {
    pub ports: HashMap<u16, PortConfig>,
    #[serde(default)]
    pub global: GlobalConfig,
    #[serde(default)]
    pub certs: HashMap<String, CertEntry>,
}

/// Per-port configuration with multi-layer flow routing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortConfig {
    #[serde(default)]
    pub listen: ListenConfig,
    pub l4: FlowNode,
    #[serde(default)]
    pub l5: Option<L5Config>,
    #[serde(default)]
    pub l7: Option<L7Config>,
}

/// A single node in the flow execution tree.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FlowNode {
    pub plugin: String,
    #[serde(default)]
    pub params: serde_json::Value,
    #[serde(default)]
    pub branches: HashMap<String, Self>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub termination: Option<TerminationAction>,
}

/// What happens when a terminator finishes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TerminationAction {
    Finished,
    Upgrade { target_layer: Layer },
}

/// Network processing layers.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Layer {
    L4,
    L5,
    L7,
}

impl fmt::Display for Layer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::L4 => write!(f, "L4"),
            Self::L5 => write!(f, "L5"),
            Self::L7 => write!(f, "L7"),
        }
    }
}

/// TLS layer configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct L5Config {
    pub default_cert: String,
    #[serde(default)]
    pub alpn: Vec<String>,
    pub flow: FlowNode,
}

/// Application layer configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct L7Config {
    pub flow: FlowNode,
}

/// Global engine settings with sensible defaults.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GlobalConfig {
    #[serde(default = "default_max_connections")]
    pub max_connections: usize,
    #[serde(default = "default_max_connections_per_ip")]
    pub max_connections_per_ip: usize,
    #[serde(default = "default_flow_timeout_secs")]
    pub flow_timeout_secs: u64,
    #[serde(default = "default_peek_limit")]
    pub peek_limit: usize,
}

const fn default_max_connections() -> usize {
    10_000
}
const fn default_max_connections_per_ip() -> usize {
    50
}
const fn default_flow_timeout_secs() -> u64 {
    10
}
const fn default_peek_limit() -> usize {
    64
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            max_connections: default_max_connections(),
            max_connections_per_ip: default_max_connections_per_ip(),
            flow_timeout_secs: default_flow_timeout_secs(),
            peek_limit: default_peek_limit(),
        }
    }
}

/// Listen configuration for a port.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ListenConfig {
    #[serde(default = "default_address")]
    pub address: String,
    #[serde(default)]
    pub ipv6: bool,
}

fn default_address() -> String {
    "0.0.0.0".to_owned()
}

impl Default for ListenConfig {
    fn default() -> Self {
        Self {
            address: default_address(),
            ipv6: false,
        }
    }
}

/// Certificate storage: file paths or inline PEM.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CertEntry {
    File {
        cert_path: String,
        key_path: String,
    },
    Pem {
        cert_pem: String,
        key_pem: String,
    },
}

/// Partial update to apply over an existing `ConfigTable`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ConfigPatch {
    pub ports: Option<HashMap<u16, PortConfig>>,
    pub global: Option<GlobalConfig>,
    pub certs: Option<HashMap<String, CertEntry>>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn sample_terminator() -> FlowNode {
        FlowNode {
            plugin: "tcp.forward".to_owned(),
            params: serde_json::json!({"ip": "127.0.0.1", "port": 8080}),
            branches: HashMap::new(),
            termination: Some(TerminationAction::Finished),
        }
    }

    fn sample_middleware(branches: HashMap<String, FlowNode>) -> FlowNode {
        FlowNode {
            plugin: "echo.branch".to_owned(),
            params: serde_json::Value::default(),
            branches,
            termination: None,
        }
    }

    fn sample_config() -> ConfigTable {
        let l4 = sample_middleware(HashMap::from([(
            "default".to_owned(),
            sample_terminator(),
        )]));

        let l5 = L5Config {
            default_cert: "main".to_owned(),
            alpn: vec!["h2".to_owned(), "http/1.1".to_owned()],
            flow: sample_terminator(),
        };

        let l7 = L7Config {
            flow: sample_terminator(),
        };

        ConfigTable {
            ports: HashMap::from([(
                443,
                PortConfig {
                    listen: ListenConfig::default(),
                    l4,
                    l5: Some(l5),
                    l7: Some(l7),
                },
            )]),
            global: GlobalConfig::default(),
            certs: HashMap::from([(
                "main".to_owned(),
                CertEntry::File {
                    cert_path: "/etc/ssl/cert.pem".to_owned(),
                    key_path: "/etc/ssl/key.pem".to_owned(),
                },
            )]),
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
        assert_eq!(listen.address, "0.0.0.0");
        assert!(!listen.ipv6);
    }

    #[test]
    fn layer_display() {
        assert_eq!(Layer::L4.to_string(), "L4");
        assert_eq!(Layer::L5.to_string(), "L5");
        assert_eq!(Layer::L7.to_string(), "L7");
    }

    #[test]
    fn cert_entry_pem_variant() {
        let entry = CertEntry::Pem {
            cert_pem: "CERT".to_owned(),
            key_pem: "KEY".to_owned(),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let back: CertEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, back);
    }

    #[test]
    fn termination_action_serde() {
        let upgrade = TerminationAction::Upgrade {
            target_layer: Layer::L5,
        };
        let json = serde_json::to_string(&upgrade).unwrap();
        assert!(json.contains("\"type\":\"upgrade\""));
        let back: TerminationAction = serde_json::from_str(&json).unwrap();
        assert_eq!(upgrade, back);

        let finished = TerminationAction::Finished;
        let json = serde_json::to_string(&finished).unwrap();
        let back: TerminationAction = serde_json::from_str(&json).unwrap();
        assert_eq!(finished, back);
    }

    #[test]
    fn config_patch_defaults_to_none() {
        let patch = ConfigPatch::default();
        assert!(patch.ports.is_none());
        assert!(patch.global.is_none());
        assert!(patch.certs.is_none());
    }

    #[test]
    fn flow_node_skip_serializing_none_termination() {
        let node = FlowNode {
            plugin: "test".to_owned(),
            params: serde_json::Value::default(),
            branches: HashMap::new(),
            termination: None,
        };
        let json = serde_json::to_string(&node).unwrap();
        assert!(!json.contains("termination"));
    }
}
