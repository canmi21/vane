use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// A single step in the flow execution tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowStep {
    pub plugin: String,
    #[serde(default)]
    pub config: StepConfig,
}

/// Configuration for a flow step: arbitrary params + named branches to child steps.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StepConfig {
    #[serde(default)]
    pub params: serde_json::Value,
    #[serde(default)]
    pub branches: HashMap<String, FlowStep>,
}

/// Maps listening ports to their root flow steps.
pub struct FlowTable {
    flows: HashMap<u16, FlowStep>,
}

impl Default for FlowTable {
    fn default() -> Self {
        Self::new()
    }
}

impl FlowTable {
    pub fn new() -> Self {
        Self {
            flows: HashMap::new(),
        }
    }

    #[must_use]
    pub fn add(mut self, port: u16, step: FlowStep) -> Self {
        self.flows.insert(port, step);
        self
    }

    pub fn lookup(&self, port: u16) -> Option<&FlowStep> {
        self.flows.get(&port)
    }

    pub fn ports(&self) -> impl Iterator<Item = u16> + '_ {
        self.flows.keys().copied()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn sample_step(plugin: &str) -> FlowStep {
        FlowStep {
            plugin: plugin.to_owned(),
            config: StepConfig::default(),
        }
    }

    #[test]
    fn flow_step_serde_roundtrip() {
        let step = FlowStep {
            plugin: "tcp.forward".to_owned(),
            config: StepConfig {
                params: serde_json::json!({"ip": "127.0.0.1", "port": 8080}),
                branches: HashMap::new(),
            },
        };
        let json = serde_json::to_string(&step).unwrap();
        let back: FlowStep = serde_json::from_str(&json).unwrap();
        assert_eq!(back.plugin, "tcp.forward");
    }

    #[test]
    fn step_config_serde_with_branches() {
        let step = FlowStep {
            plugin: "echo.branch".to_owned(),
            config: StepConfig {
                params: serde_json::json!({"branch": "default"}),
                branches: HashMap::from([(
                    "default".to_owned(),
                    sample_step("tcp.forward"),
                )]),
            },
        };
        let json = serde_json::to_string(&step).unwrap();
        let back: FlowStep = serde_json::from_str(&json).unwrap();
        assert_eq!(back.config.branches.len(), 1);
        assert!(back.config.branches.contains_key("default"));
    }

    #[test]
    fn step_config_defaults() {
        let json = r#"{"plugin":"test"}"#;
        let step: FlowStep = serde_json::from_str(json).unwrap();
        assert!(step.config.params.is_null());
        assert!(step.config.branches.is_empty());
    }

    #[test]
    fn flow_table_add_lookup_ports() {
        let table = FlowTable::new()
            .add(80, sample_step("a"))
            .add(443, sample_step("b"));

        assert!(table.lookup(80).is_some());
        assert!(table.lookup(443).is_some());
        assert!(table.lookup(8080).is_none());

        let mut ports: Vec<u16> = table.ports().collect();
        ports.sort_unstable();
        assert_eq!(ports, vec![80, 443]);
    }
}
