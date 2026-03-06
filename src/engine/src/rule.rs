use std::collections::HashMap;
use std::net::SocketAddr;

use vane_transport::tcp::ProxyConfig;

pub struct ForwardRule {
    pub upstream: SocketAddr,
    pub proxy_config: ProxyConfig,
}

pub struct RouteTable {
    rules: HashMap<u16, ForwardRule>,
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

    pub fn add(mut self, port: u16, rule: ForwardRule) -> Self {
        self.rules.insert(port, rule);
        self
    }

    pub fn lookup(&self, port: u16) -> Option<&ForwardRule> {
        self.rules.get(&port)
    }

    pub fn ports(&self) -> impl Iterator<Item = u16> + '_ {
        self.rules.keys().copied()
    }
}
