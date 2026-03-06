use std::collections::HashMap;

use vane_primitives::model::Forward;
use vane_transport::tcp::ProxyConfig;

pub struct PortRule {
    pub forward: Forward,
    pub proxy_config: ProxyConfig,
}

pub struct RouteTable {
    rules: HashMap<u16, PortRule>,
}

impl Default for RouteTable {
    fn default() -> Self {
        Self::new()
    }
}

impl RouteTable {
    pub fn new() -> Self {
        Self {
            rules: HashMap::new(),
        }
    }

    pub fn add(mut self, port: u16, rule: PortRule) -> Self {
        self.rules.insert(port, rule);
        self
    }

    pub fn lookup(&self, port: u16) -> Option<&PortRule> {
        self.rules.get(&port)
    }

    pub fn ports(&self) -> impl Iterator<Item = u16> + '_ {
        self.rules.keys().copied()
    }
}
