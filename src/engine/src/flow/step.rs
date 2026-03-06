use std::collections::HashMap;

use crate::config::FlowNode;

/// Maps listening ports to their root flow nodes.
pub struct FlowTable {
    flows: HashMap<u16, FlowNode>,
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
    pub fn add(mut self, port: u16, node: FlowNode) -> Self {
        self.flows.insert(port, node);
        self
    }

    pub fn lookup(&self, port: u16) -> Option<&FlowNode> {
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

    fn sample_node(plugin: &str) -> FlowNode {
        FlowNode {
            plugin: plugin.to_owned(),
            params: serde_json::Value::default(),
            branches: HashMap::new(),
            termination: None,
        }
    }

    #[test]
    fn flow_table_add_lookup_ports() {
        let table = FlowTable::new()
            .add(80, sample_node("a"))
            .add(443, sample_node("b"));

        assert!(table.lookup(80).is_some());
        assert!(table.lookup(443).is_some());
        assert!(table.lookup(8080).is_none());

        let mut ports: Vec<u16> = table.ports().collect();
        ports.sort_unstable();
        assert_eq!(ports, vec![80, 443]);
    }
}
