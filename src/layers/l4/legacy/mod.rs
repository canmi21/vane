/* src/layers/l4/legacy/mod.rs */

//! Legacy L4 Transport Configuration System (Preserved Feature)
//!
//! This module contains the traditional priority-based protocol detection
//! configuration system that predates the flow-based architecture.
//!
//! **Status**: Preserved for backward compatibility, no future updates.
//! **Supported Layers**: L4 Transport only (L4+ and L7 do not support legacy config)
//!
//! Users should migrate to flow-based configuration for new deployments.

pub mod tcp;
pub mod udp;

pub use tcp::{LegacyTcpConfig, dispatch_legacy_tcp, validate_tcp_rules};
pub use udp::{LegacyUdpConfig, dispatch_legacy_udp, validate_udp_rules};
