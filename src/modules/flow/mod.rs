/* src/modules/flow/mod.rs */

//! Unified Flow Execution Engine
//!
//! This module provides a unified flow execution engine used by all three layers:
//! - L4 Transport
//! - L4+ Carrier
//! - L7 Application
//!
//! # Architecture
//!
//! The flow engine follows a recursive execution model where each step consists of
//! exactly one plugin. Plugins can be either Middlewares (which branch to next steps)
//! or Terminators (which finish the connection or request).
//!
//! Differences between layers (L4, L4+, L7) are abstracted through the `ExecutionContext` trait.
//! - `TransportContext` provides access to the KV Store (used by L4/L4+).
//! - `ApplicationContext` provides access to the full `Container` (used by L7).
//!
//! # Execution Priority
//!
//! For the L7 layer, the engine automatically handles L7-specific plugin traits:
//! 1. Try `L7Middleware` first, then fallback to `Middleware`.
//! 2. Try `L7Terminator` first, then fallback to `Terminator`.

pub mod context;
pub mod engine;
pub mod key_scoping;

pub use context::{ApplicationContext, ExecutionContext, TransportContext};
pub use engine::{execute, execute_l7};
