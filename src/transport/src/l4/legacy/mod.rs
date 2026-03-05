/* src/transport/src/l4/legacy/mod.rs */

//! Legacy L4 Transport Configuration System (Preserved Feature)
//!
//! Type definitions live in vane-engine; dispatch functions stay here
//! until they move to vane-transport in Step 5.

pub mod tcp;
pub mod udp;

pub use tcp::{LegacyTcpConfig, dispatch_legacy_tcp, validate_tcp_rules};
pub use udp::{LegacyUdpConfig, dispatch_legacy_udp, validate_udp_rules};
