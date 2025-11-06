/* engine/src/modules/ratelimit/mod.rs */

pub mod gc;
pub mod heap;
pub mod manager;
pub mod pool;

// Re-export the public-facing functions for easy access from other parts of the engine.
pub use pool::{check, start_gc_task};
