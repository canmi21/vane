/* src/config/types.rs */

use live::loader::PreProcess;

// Re-export existing config types
pub use crate::layers::l4::tcp::TcpConfig;
pub use crate::layers::l4::udp::UdpConfig;
pub use crate::layers::l4p::model::ResolverConfig;
pub use crate::layers::l7::model::ApplicationConfig;
pub use crate::lazycert::config::LazyCertConfig;
pub use crate::resources::certs::arcswap::LoadedCert as CertEntry;
pub use crate::resources::service_discovery::model::NodesConfig;

// Implement PreProcess for config types
impl PreProcess for TcpConfig {
	fn pre_process(&mut self) {
		if let Self::Legacy(config) = self {
			for rule in &mut config.rules {
				rule.name = rule.name.to_lowercase();
			}
		}
	}
}

impl PreProcess for UdpConfig {
	fn pre_process(&mut self) {
		if let Self::Legacy(config) = self {
			for rule in &mut config.rules {
				rule.name = rule.name.to_lowercase();
			}
		}
	}
}
impl PreProcess for LazyCertConfig {
	fn pre_process(&mut self) {
		self.url = self.url.trim_end_matches('/').to_owned();
	}
}

impl PreProcess for ResolverConfig {
	fn set_context(&mut self, ctx: &str) {
		self.protocol = ctx.to_owned();
	}
}

impl PreProcess for ApplicationConfig {
	fn set_context(&mut self, ctx: &str) {
		self.protocol = ctx.to_owned();
	}
}
