// Re-export core trait and TransportContext from engine crate
pub use vane_engine::engine::context::{ExecutionContext, TransportContext};

// ApplicationContext now lives in vane-app
pub use vane_app::context::ApplicationContext;
