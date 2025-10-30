/* engine/src/modules/plugins/mod.rs */

pub mod builtin;
pub mod handler;
pub mod manager;

// Publicly export the primary data structures for use in other parts of the application.
pub use manager::{Plugin, PluginInterface};
