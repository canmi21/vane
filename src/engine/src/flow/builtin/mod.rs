pub mod echo_branch;
pub mod protocol_detect;
pub mod tcp_forward;
pub mod tls_clienthello;

use vane_transport::tcp::ProxyConfig;

use crate::flow::plugin::PluginAction;
use crate::flow::registry::PluginRegistry;

use self::echo_branch::EchoBranch;
use self::protocol_detect::ProtocolDetect;
use self::tcp_forward::TcpForward;
use self::tls_clienthello::TlsClientHello;

#[must_use]
pub fn default_plugin_registry() -> PluginRegistry {
	PluginRegistry::new()
		.register("echo.branch", PluginAction::Middleware(Box::new(EchoBranch)))
		.register(
			"protocol.detect",
			PluginAction::Middleware(Box::new(ProtocolDetect::with_defaults())),
		)
		.register("tls.clienthello", PluginAction::Middleware(Box::new(TlsClientHello)))
		.register(
			"tcp.forward",
			PluginAction::Terminator(Box::new(TcpForward { proxy_config: ProxyConfig::default() })),
		)
}
