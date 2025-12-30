/* src/modules/stack/transport/loader.rs */

pub use crate::common::loader::{PreProcess, load_config, load_file};

use super::{tcp::TcpConfig, udp::UdpConfig};

// Implement PreProcess for the new enum. It only applies to the legacy variant.
impl PreProcess for TcpConfig {
	fn pre_process(&mut self) {
		if let TcpConfig::Legacy(config) = self {
			for rule in &mut config.rules {
				rule.name = rule.name.to_lowercase();
			}
		}
	}
}

impl PreProcess for UdpConfig {
	fn pre_process(&mut self) {
		if let UdpConfig::Legacy(config) = self {
			for rule in &mut config.rules {
				rule.name = rule.name.to_lowercase();
			}
		}
	}
}
