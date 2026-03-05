// API module now lives in vane-api
pub use vane_api::*;

// Re-export sub-modules for backward compatibility
pub mod handlers {
	pub use vane_api::handlers::*;
}
pub mod middleware {
	pub use vane_api::middleware::*;
}
pub mod openapi {
	pub use vane_api::openapi::*;
}
pub mod response {
	pub use vane_api::response::*;
}
pub mod router {
	pub use vane_api::router::*;
}
pub mod schemas {
	pub use vane_api::schemas::*;
}
pub mod utils {
	pub use vane_api::utils::*;
}
